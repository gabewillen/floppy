//! The `BasicHTTP` network of live sessions (SCXML 1.0 appendix C.2) over
//! real local sockets: the allowlisted [`NetworkClient`] posting to an
//! in-test HTTP server, late `error.communication` for failures after
//! acceptance (non-2xx, redirect, refused connection, timeout), allowlist
//! denials (dot segments, `%2e%2e`, backslashes, userinfo), and the built-in
//! [`BasicHttpListener`] delivering inbound messages to the root and to an
//! invoked session, and record and replay of a networked session.
//!
//! `scripts/verify-wasm-capabilities.sh` builds and verifies the release
//! components into `target/session-host-proof` (this suite's own artifacts
//! directory inside it).

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]
#![cfg(feature = "http")]

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use scxml_host_wasmtime::{
    Applied, BasicHttpListener, CommunicationSender, DocumentKind, EmbeddedDocument, HttpAllowlist,
    HttpClientConfig, InboundHttp, InputRecord, ListenerConfig, ListenerHandle, LiveClock,
    LiveHttp, LiveSession, LiveStatus, MachineScripts, NetworkClient, PublicUrl, ReleaseComponents,
    SessionEmbedder, SessionHost, SessionOpen, SessionOutput, TelemetryProviders,
    TrustedManifestDigest, WasmtimeHostConfig, basichttp_types, scxml_snapshot,
};
use sha2::{Digest as _, Sha256};

/// The source id of the documents these tests parse.
const SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);

const COMPONENTS: [&str; 8] = [
    "scxml_machine",
    "scxml_compiler",
    "scxml_data_model_ecmascript",
    "scxml_data_model_xpath",
    "scxml_data_model_null",
    "scxml_ioprocessor_scxml",
    "scxml_ioprocessor_basichttp",
    "scxml_clock",
];

const BASICHTTP: &str = "http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor";

fn components() -> &'static ReleaseComponents {
    static COMPONENTS_CELL: OnceLock<ReleaseComponents> = OnceLock::new();
    COMPONENTS_CELL.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("host crate has a workspace parent");
        let target = workspace.join("target/session-host-proof");
        let release = target.join("wasm-artifacts/basichttp-network");
        let status = Command::new("sh")
            .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
            .arg("--artifacts-dir")
            .arg(&release)
            .args(COMPONENTS)
            .current_dir(workspace)
            .env("CARGO_TARGET_DIR", &target)
            .status()
            .expect("the capability script runs");
        assert!(status.success(), "the release capability proof must pass");
        let manifest = std::fs::read(release.join("manifest.tsv")).expect("manifest is readable");
        let trusted = TrustedManifestDigest::from_hex(&format!("{:x}", Sha256::digest(&manifest)))
            .expect("manifest digest is canonical");
        ReleaseComponents::load(&release, trusted)
            .unwrap_or_else(|error| panic!("the release components load: {error}"))
    })
}

fn host() -> SessionHost {
    SessionHost::new(
        &components().session_components(),
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
    .expect("the verified components link")
}

struct Documents(Vec<(&'static str, String)>);

impl SessionEmbedder for Documents {
    fn document(&self, _kind: DocumentKind, uri: &str) -> Option<EmbeddedDocument> {
        self.0
            .iter()
            .find(|(key, _)| *key == uri)
            .map(|(_, text)| text.clone().into())
    }

    fn scripts(&self) -> MachineScripts {
        MachineScripts::none()
    }
}

fn scxml(body: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0">{body}</scxml>"#
    )
}

fn open(
    host: &SessionHost,
    source: &str,
    documents: Vec<(&'static str, String)>,
    http: LiveHttp,
) -> LiveSession {
    open_with(host, source, documents, http, Recording::Off)
}

/// Whether a live session records its inputs.
#[derive(Clone, Copy, PartialEq)]
enum Recording {
    On,
    Off,
}

fn open_with(
    host: &SessionHost,
    source: &str,
    documents: Vec<(&'static str, String)>,
    http: LiveHttp,
    recording: Recording,
) -> LiveSession {
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        "network.scxml",
        source,
    ))
    .expect("fixture parses");
    let ir = scxml_compiler_core::lower(&document).expect("fixture lowers");
    host.open(SessionOpen {
        machine_ir: &ir.to_json().expect("fixture IR encodes"),
        data_model: ir.data_model.as_deref(),
        session_id: "root",
        embedder: Rc::new(Documents(documents)),
        max_commands: 1_024,
        clock: LiveClock::Logical,
        record: recording == Recording::On,
        replay: None,
        http,
    })
    .expect("the session opens")
}

/// Opens `source` as the root `root` with `recording` and `replay`.
fn open_recorded(
    host: &SessionHost,
    source: &str,
    recording: Recording,
    replay: Option<&[InputRecord]>,
    http: LiveHttp,
) -> LiveSession {
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        "network.scxml",
        source,
    ))
    .expect("fixture parses");
    let ir = scxml_compiler_core::lower(&document).expect("fixture lowers");
    host.open(SessionOpen {
        machine_ir: &ir.to_json().expect("fixture IR encodes"),
        data_model: ir.data_model.as_deref(),
        session_id: "root",
        embedder: Rc::new(Documents(Vec::new())),
        max_commands: 1_024,
        clock: LiveClock::Logical,
        record: recording == Recording::On,
        replay,
        http,
    })
    .expect("the session opens")
}

/// One request as an in-test server received it.
#[derive(Debug)]
struct Received {
    head: String,
    body: String,
}

/// What the in-test server does with each request.
#[derive(Clone, Copy)]
enum Reply {
    Status(u16),
    Redirect,
    Stall(Duration),
}

/// A one-thread HTTP/1.1 server that records every request it reads.
struct TestServer {
    addr: SocketAddr,
    received: mpsc::Receiver<Received>,
}

impl TestServer {
    fn start(reply: Reply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a test port");
        let addr = listener.local_addr().expect("local address");
        let (sender, received) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let Some(request) = read_request(&mut stream) else {
                    continue;
                };
                let _ = sender.send(request);
                let response = match reply {
                    Reply::Status(status) => {
                        format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\n\r\n")
                    }
                    Reply::Redirect => format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{addr}/elsewhere\r\nContent-Length: 0\r\n\r\n"
                    ),
                    Reply::Stall(time) => {
                        std::thread::sleep(time);
                        "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_owned()
                    }
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self { addr, received }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
}

/// Reads one request head and its `Content-Length` body.
fn read_request(stream: &mut TcpStream) -> Option<Received> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];
    let head_end = loop {
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8(bytes[..head_end].to_vec()).ok()?;
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    let mut body = bytes[head_end..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    Some(Received {
        head,
        body: String::from_utf8(body).ok()?,
    })
}

/// A network with `allow` and a wake counter.
fn network(allow: &[&str], total_timeout: Duration) -> (LiveHttp, mpsc::Receiver<()>) {
    let mut config = HttpClientConfig::new(HttpAllowlist::new(allow).expect("valid allowlist"));
    config.total_timeout = total_timeout;
    config.connect_timeout = total_timeout;
    let (wake, woken) = mpsc::channel();
    let wake = std::sync::Mutex::new(wake);
    (
        LiveHttp {
            public_url: None,
            outbound: Some(Arc::new(NetworkClient::new(config).expect("valid client"))),
            on_communication_error: Some(Arc::new(move || {
                let _ = wake.lock().map(|wake| wake.send(()));
            })),
        },
        woken,
    )
}

/// Waits until every accepted request settled, then delivers what is owed.
fn settle(live: &mut LiveSession) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while live.communication_pending() {
        live.deliver_communication_errors()
            .expect("owed errors deliver");
        assert!(Instant::now() < deadline, "requests settle in time");
        std::thread::sleep(Duration::from_millis(10));
    }
    live.deliver_communication_errors()
        .expect("owed errors deliver");
}

/// A root that POSTs to `target` on entry and reaches `lost` only on an
/// `error.communication` the platform raised (`_event.type` `platform`,
/// SCXML 5.10.1); any other type of it reaches `mistyped`.
fn send_to(target: &str) -> String {
    scxml(&format!(
        r#"<state id="s0"><onentry><send event="notify" type="{BASICHTTP}" target="{target}"><param name="answer" expr="42"/><param name="text" expr="'a b&amp;c'"/></send></onentry><transition event="error.communication" cond="_event.type == 'platform'" target="lost"/><transition event="error.communication" target="mistyped"/></state><final id="lost"/><final id="mistyped"/>"#
    ))
}

#[test]
fn an_allowed_post_reaches_the_server_in_exact_c2_form() {
    let host = host();
    let server = TestServer::start(Reply::Status(200));
    let (http, woken) = network(&[&server.url("/hooks")], Duration::from_secs(10));
    let mut live = open(
        &host,
        &send_to(&server.url("/hooks/agent")),
        Vec::new(),
        http,
    );
    // The send was accepted without waiting for the network.
    assert_eq!(live.status(), LiveStatus::Running);
    let request = server
        .received
        .recv_timeout(Duration::from_secs(10))
        .expect("the server receives the POST");
    let request_line = request.head.lines().next().unwrap_or_default();
    assert_eq!(request_line, "POST /hooks/agent HTTP/1.1");
    assert!(
        request
            .head
            .to_ascii_lowercase()
            .contains("content-type: application/x-www-form-urlencoded\r\n"),
        "{}",
        request.head
    );
    for forbidden in ["cookie:", "authorization:", "accept-encoding:"] {
        assert!(!request.head.to_ascii_lowercase().contains(forbidden));
    }
    assert_eq!(
        request.body,
        "_scxmleventname=notify&answer=42&text=a%20b%26c"
    );
    woken
        .recv_timeout(Duration::from_secs(10))
        .expect("the settled request wakes the embedder");
    settle(&mut live);
    assert_eq!(live.status(), LiveStatus::Running, "a 2xx owes nothing");
    assert_eq!(live.configuration().expect("configuration"), ["s0"]);
    live.close().expect("close");
}

#[test]
fn failures_after_acceptance_raise_error_communication_later() {
    let host = host();
    for (reply, timeout) in [
        (Reply::Status(500), Duration::from_secs(10)),
        (Reply::Status(404), Duration::from_secs(10)),
        (Reply::Redirect, Duration::from_secs(10)),
        (
            Reply::Stall(Duration::from_secs(3)),
            Duration::from_millis(300),
        ),
    ] {
        let server = TestServer::start(reply);
        let (http, _woken) = network(&[&server.url("/")], timeout);
        let mut live = open(&host, &send_to(&server.url("/hook")), Vec::new(), http);
        assert_eq!(live.status(), LiveStatus::Running, "accepted at once");
        settle(&mut live);
        assert_eq!(
            live.status(),
            LiveStatus::Finished,
            "late error.communication"
        );
        assert_eq!(live.configuration().expect("configuration"), ["lost"]);
        // A redirect is never followed.
        assert!(server.received.recv_timeout(Duration::from_secs(5)).is_ok());
        assert!(
            server
                .received
                .recv_timeout(Duration::from_millis(200))
                .is_err()
        );
        live.close().expect("close");
    }

    // Nothing listens: the refused connection fails later too.
    let closed = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}/hook", closed.local_addr().expect("address"));
    drop(closed);
    let (http, _woken) = network(&[&url], Duration::from_secs(5));
    let mut live = open(&host, &send_to(&url), Vec::new(), http);
    settle(&mut live);
    assert_eq!(live.configuration().expect("configuration"), ["lost"]);
    live.close().expect("close");
}

/// A root whose `<send>` (with `identity`: an `id` or `idlocation`
/// attribute, or nothing) POSTs to `target` on entry and reaches `lost`
/// only on an `error.communication` whose `_event.sendid` satisfies
/// `expected` (SCXML 5.10.1: the send id of the triggering `<send>`);
/// any other `error.communication` reaches `unnamed`.
fn identified_send_to(target: &str, identity: &str, expected: &str) -> String {
    scxml(&format!(
        r#"<datamodel><data id="sid"/></datamodel><state id="s0"><onentry><send {identity} event="notify" type="{BASICHTTP}" target="{target}"/></onentry><transition event="error.communication" cond="{expected}" target="lost"/><transition event="error.communication" target="unnamed"/></state><final id="lost"/><final id="unnamed"/>"#
    ))
}

/// The `<send>` identities under test and the `_event.sendid` condition
/// each must satisfy: the authored id, the id stored at `idlocation`, and
/// the platform-generated id of a send with neither (SCXML 3.14).
const IDENTITIES: [(&str, &str); 3] = [
    (r#"id="note""#, "_event.sendid == 'note'"),
    (
        r#"idlocation="sid""#,
        "typeof sid == 'string' &amp;&amp; _event.sendid == sid",
    ),
    (
        "",
        "typeof _event.sendid == 'string' &amp;&amp; _event.sendid.length &gt; 0",
    ),
];

#[test]
fn a_late_failure_names_the_failed_send() {
    let host = host();
    let server = TestServer::start(Reply::Status(500));
    for (identity, expected) in IDENTITIES {
        let (http, _woken) = network(&[&server.url("/")], Duration::from_secs(10));
        let mut live = open(
            &host,
            &identified_send_to(&server.url("/hook"), identity, expected),
            Vec::new(),
            http,
        );
        assert_eq!(live.status(), LiveStatus::Running, "accepted at once");
        settle(&mut live);
        assert_eq!(
            live.configuration().expect("configuration"),
            ["lost"],
            "{identity}: the late error.communication carries the send id"
        );
        live.close().expect("close");
    }
}

#[test]
fn an_immediate_refusal_names_the_failed_send() {
    let host = host();
    for (identity, expected) in IDENTITIES {
        let (http, _woken) = network(&[], Duration::from_secs(5));
        let mut live = open(
            &host,
            &identified_send_to("https://denied.example/hook", identity, expected),
            Vec::new(),
            http,
        );
        assert_eq!(
            live.configuration().expect("configuration"),
            ["lost"],
            "{identity}: the refusal's error.communication carries the send id"
        );
        live.close().expect("close");
    }
}

#[test]
fn an_invoked_sessions_late_failure_names_its_send() {
    let host = host();
    let server = TestServer::start(Reply::Status(503));
    let child = scxml(&format!(
        r##"<state id="s0"><onentry><send id="child-note" event="notify" type="{BASICHTTP}" target="{}"/></onentry><transition event="error.communication" cond="_event.sendid == 'child-note' &amp;&amp; _event.type == 'platform'" target="lost"/><transition event="error.communication" target="unnamed"/></state><final id="lost"><onentry><send event="named" target="#_parent"/></onentry></final><final id="unnamed"/>"##,
        server.url("/hook")
    ));
    let source = scxml(
        r#"<state id="s0"><invoke type="scxml" src="child.scxml"/><transition event="named" target="done"/><transition event="error.*" target="leaked"/></state><final id="done"/><final id="leaked"/>"#,
    );
    let (http, _woken) = network(&[&server.url("/")], Duration::from_secs(10));
    let mut live = open(&host, &source, vec![("child.scxml", child)], http);
    assert_eq!(live.status(), LiveStatus::Running);
    settle(&mut live);
    assert_eq!(
        live.configuration().expect("configuration"),
        ["done"],
        "the child's own error names its send, and the parent sees none"
    );
    live.close().expect("close");
}

#[test]
fn denied_targets_fail_at_once_and_send_nothing() {
    let host = host();
    let server = TestServer::start(Reply::Status(200));
    let prefix = server.url("/scxml");
    for target in [
        server.url("/scxml/../admin"),
        server.url("/scxml/%2e%2e/admin"),
        server.url("/scxml/%2E%2E/admin"),
        server.url("/scxml\\..\\admin"),
        server.url("/scxml-admin"),
        format!("http://user:secret@{}/scxml/x", server.addr),
        "http://api.example.invalid/scxml".to_owned(),
    ] {
        let (http, _woken) = network(&[&prefix], Duration::from_secs(5));
        let mut live = open(&host, &send_to(&target), Vec::new(), http);
        assert_eq!(
            live.configuration().expect("configuration"),
            ["lost"],
            "{target} is denied at once"
        );
        assert!(!live.communication_pending());
        live.close().expect("close");
    }
    // Without any allowlist entry nothing is sent either.
    let (http, _woken) = network(&[], Duration::from_secs(5));
    let mut live = open(&host, &send_to(&server.url("/scxml/x")), Vec::new(), http);
    assert_eq!(live.configuration().expect("configuration"), ["lost"]);
    live.close().expect("close");
    assert!(
        server
            .received
            .recv_timeout(Duration::from_millis(300))
            .is_err(),
        "no denied request reached the server"
    );
}

/// A running listener whose inbound messages arrive on a channel; it
/// stops when dropped.
struct Inbound {
    addr: SocketAddr,
    messages: mpsc::Receiver<InboundHttp>,
    /// Keeps the listener serving.
    _listening: ListenerHandle,
}

fn listen() -> Inbound {
    let listener = BasicHttpListener::bind(
        "127.0.0.1:0".parse().expect("address"),
        ListenerConfig::default(),
    )
    .expect("bind the listener");
    let addr = listener.local_addr();
    let (sender, messages) = mpsc::channel();
    let sender = std::sync::Mutex::new(sender);
    let listening = listener
        .serve(move |inbound| {
            let _ = sender.lock().map(|sender| sender.send(inbound));
        })
        .expect("serve");
    Inbound {
        addr,
        messages,
        _listening: listening,
    }
}

/// Sends raw request bytes; returns the response status.
fn client(addr: SocketAddr, request: Vec<u8>) -> std::thread::JoinHandle<u16> {
    std::thread::spawn(move || {
        let mut stream = TcpStream::connect(addr).expect("connect");
        stream.write_all(&request).expect("write");
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        response
            .split(' ')
            .nth(1)
            .and_then(|status| status.parse().ok())
            .unwrap_or(0)
    })
}

fn post(path: &str, content_type: &str, body: &str) -> Vec<u8> {
    format!(
        "POST {path} HTTP/1.1\r\nHost: test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// Serves one inbound message on the session thread.
fn serve_one(
    live: &mut LiveSession,
    inbound: &Inbound,
) -> Option<scxml_host_wasmtime::LiveCallResult> {
    let message = inbound
        .messages
        .recv_timeout(Duration::from_secs(10))
        .expect("the listener delivers the message")
        .claim()
        .expect("claimed in time");
    let (response, step) = match message.request() {
        Ok(request) => {
            let receipt = live.receive_http(request);
            (receipt.response, receipt.result.expect("the step succeeds"))
        }
        Err(rejection) => (
            live.http_rejection(rejection)
                .expect("the component answers"),
            None,
        ),
    };
    message.respond(response);
    step
}

fn logs(outputs: &[SessionOutput]) -> Vec<(String, String)> {
    outputs
        .iter()
        .filter_map(|output| match output {
            SessionOutput::Log { session, entry, .. } => {
                Some((session.clone(), entry.text.clone()))
            }
            SessionOutput::Done { .. } => None,
        })
        .collect()
}

#[test]
fn the_listener_delivers_inbound_messages_to_the_root() {
    let host = host();
    let inbound = listen();
    let public = format!("http://{}", inbound.addr);
    let http = LiveHttp {
        public_url: Some(PublicUrl::new(&public).expect("public URL")),
        ..LiveHttp::default()
    };
    let source = scxml(
        r#"<state id="s0"><onentry><log expr="_ioprocessors['basichttp'].location"/></onentry><transition event="go" target="done"><log expr="_event.data.x + '|' + _event.origintype"/></transition></state><final id="done"/>"#,
    );
    let mut live = open(&host, &source, Vec::new(), http);
    let location = format!("{public}/sessions/root/basichttp");
    assert_eq!(
        live.io_processors().expect("io processors").get(BASICHTTP),
        Some(&location)
    );
    assert_eq!(logs(&live.take_outputs()), [("root".to_owned(), location)]);

    // Wrong method, unknown session, and an over-long body enqueue nothing.
    for (request, status) in [
        (
            b"GET /sessions/root/basichttp HTTP/1.1\r\nHost: t\r\n\r\n".to_vec(),
            405,
        ),
        (
            post("/sessions/nobody/basichttp", "text/plain", "x"),
            404,
        ),
        (
            format!(
                "POST /sessions/root/basichttp HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
                70_000
            )
            .into_bytes(),
            413,
        ),
        (
            b"POST /sessions/root/basichttp HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"
                .to_vec(),
            400,
        ),
    ] {
        let answer = client(inbound.addr, request);
        assert_eq!(serve_one(&mut live, &inbound), None);
        assert_eq!(answer.join().expect("client"), status);
    }
    assert_eq!(live.configuration().expect("configuration"), ["s0"]);

    let answer = client(
        inbound.addr,
        post(
            "/sessions/root/basichttp",
            "application/x-www-form-urlencoded",
            "_scxmleventname=go&x=1",
        ),
    );
    let step = serve_one(&mut live, &inbound).expect("the event ran a step");
    assert_eq!(answer.join().expect("client"), 200);
    assert_eq!(step.status, LiveStatus::Finished);
    assert_eq!(
        logs(&live.take_outputs()),
        [("root".to_owned(), format!("1|{BASICHTTP}"))]
    );
    // A finished session answers nothing.
    let receipt = live.receive_http(&basichttp_types::HttpRequest {
        method: "POST".into(),
        uri: "/sessions/root/basichttp".into(),
        headers: Vec::new(),
        body: Vec::new(),
    });
    assert_eq!(receipt.response.status, 404);
    live.close().expect("close");
}

#[test]
fn inbound_messages_reach_an_invoked_session_at_its_own_location() {
    let host = host();
    let inbound = listen();
    let public = format!("http://{}/agent", inbound.addr);
    let http = LiveHttp {
        public_url: Some(PublicUrl::new(&public).expect("public URL")),
        ..LiveHttp::default()
    };
    let grandchild = scxml(
        r##"<state id="s0"><onentry><log expr="_ioprocessors['basichttp'].location"/></onentry><transition event="ping" target="done"><send event="pong" target="#_parent"><param name="value" expr="_event.data"/></send></transition></state><final id="done"/>"##,
    );
    let child = scxml(
        r##"<state id="s0"><invoke type="scxml" src="grandchild.scxml"/><transition event="pong" target="done"><send event="pong" target="#_parent"><param name="value" expr="_event.data.value"/></send></transition></state><final id="done"/>"##,
    );
    let source = scxml(
        r#"<state id="s0"><invoke type="scxml" src="child.scxml"/><transition event="pong" target="done"><log expr="'pong ' + _event.data.value"/></transition></state><final id="done"/>"#,
    );
    let mut live = open_with(
        &host,
        &source,
        vec![("child.scxml", child), ("grandchild.scxml", grandchild)],
        http,
        Recording::On,
    );
    let logged = logs(&live.take_outputs());
    let [(session, location)] = logged.as_slice() else {
        panic!("the grandchild logs its location: {logged:?}");
    };
    // The host names invoked sessions from one run-wide counter: the
    // grandchild is the run's second invoked session.
    assert_eq!(session, "root.2");
    assert_eq!(location, &format!("{public}/sessions/{session}/basichttp"));
    let path = location
        .strip_prefix(&format!("http://{}", inbound.addr))
        .expect("location under the listener");
    // SCXML C.2: the event name may travel as a header; a non-form body is
    // the event's text data.
    let answer = client(
        inbound.addr,
        format!(
            "POST {path} HTTP/1.1\r\nHost: t\r\nContent-Type: text/plain\r\n_scxmleventname: ping\r\nContent-Length: 5\r\n\r\nhello"
        )
        .into_bytes(),
    );
    let step = serve_one(&mut live, &inbound).expect("the event ran a step");
    assert_eq!(answer.join().expect("client"), 200);
    assert_eq!(step.status, LiveStatus::Finished);
    assert_eq!(
        logs(&live.take_outputs()),
        [("root".to_owned(), "pong hello".to_owned())]
    );
    // The recorded message names its session's lineage: the invoke ids
    // from the root (generated, SCXML 6.4.1 `stateid.platformid`).
    let records = live.take_recorded();
    let http: Vec<(&str, &[String])> = records
        .iter()
        .filter_map(|record| match record {
            InputRecord::Http {
                session_id, path, ..
            } => Some((session_id.as_str(), path.as_slice())),
            _ => None,
        })
        .collect();
    assert_eq!(
        http,
        [(
            "root.2",
            ["s0.root.0".to_owned(), "s0.root.1.0".to_owned()].as_slice()
        )]
    );
    live.close().expect("close");
}

#[test]
fn two_sessions_exchange_events_over_the_network() {
    let host = host();
    let inbound_a = listen();
    let inbound_b = listen();
    let url = |inbound: &Inbound| format!("http://{}", inbound.addr);
    let (mut http_a, _woken_a) = network(&[&url(&inbound_b)], Duration::from_secs(10));
    http_a.public_url = Some(PublicUrl::new(&url(&inbound_a)).expect("public URL"));
    let (mut http_b, _woken_b) = network(&[&url(&inbound_a)], Duration::from_secs(10));
    http_b.public_url = Some(PublicUrl::new(&url(&inbound_b)).expect("public URL"));
    // A pings B at B's location with its own location as the reply address;
    // B answers pong there.
    let a = scxml(&format!(
        r#"<state id="s0"><onentry><send event="ping" type="{BASICHTTP}" target="{}/sessions/root/basichttp"><param name="reply" expr="_ioprocessors['basichttp'].location"/></send></onentry><transition event="pong" target="done"><log expr="'got ' + _event.data.n"/></transition><transition event="error.communication" target="lost"/></state><final id="done"/><final id="lost"/>"#,
        url(&inbound_b)
    ));
    let b = scxml(&format!(
        r#"<state id="s0"><transition event="ping" target="done"><send event="pong" type="{BASICHTTP}" targetexpr="_event.data.reply"><param name="n" expr="7"/></send></transition><transition event="error.communication" target="lost"/></state><final id="done"/><final id="lost"/>"#
    ));
    let mut session_b = open(&host, &b, Vec::new(), http_b);
    let mut session_a = open(&host, &a, Vec::new(), http_a);
    let step = serve_one(&mut session_b, &inbound_b).expect("B processed ping");
    assert_eq!(step.status, LiveStatus::Finished);
    let step = serve_one(&mut session_a, &inbound_a).expect("A processed pong");
    assert_eq!(step.status, LiveStatus::Finished);
    assert_eq!(
        logs(&session_a.take_outputs()),
        [("root".to_owned(), "got 7".to_owned())]
    );
    settle(&mut session_b);
    assert_eq!(session_b.configuration().expect("configuration"), ["done"]);
    session_a.close().expect("close");
    session_b.close().expect("close");
}

/// A late failure is owed to the very `<send>` that made the request, in
/// an invoked session too: its `error.communication` reaches that child,
/// and the record names the child by `_sessionid` and invoke-id path, so
/// the replay delivers it there again.
#[test]
fn an_invoked_session_gets_its_late_communication_error_recorded_and_replayed() {
    let host = host();
    let server = TestServer::start(Reply::Status(500));
    let (http, _woken) = network(&[&server.url("/")], Duration::from_secs(10));
    let source = scxml(&format!(
        r#"<state id="s0"><invoke id="kid"><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="c"><state id="c"><onentry><send id="kid-note" event="notify" type="{BASICHTTP}" target="{}"/></onentry><transition event="error.communication"><log label="late" expr="_event.sendid"/></transition></state></scxml></content></invoke></state>"#,
        server.url("/hook")
    ));
    let mut live = open_recorded(&host, &source, Recording::On, None, http);
    settle(&mut live);
    let outputs = live.take_outputs();
    assert_eq!(
        logs(&outputs),
        [("root.1".to_owned(), "kid-note".to_owned())],
        "the child that sent gets the error"
    );
    let records = live.take_recorded();
    live.close().expect("close");
    let senders: Vec<&CommunicationSender> = records
        .iter()
        .filter_map(|record| match record {
            InputRecord::CommunicationErrors { senders, .. } => Some(senders),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(
        senders,
        [&CommunicationSender {
            session_id: "root.1".into(),
            path: vec!["kid".into()],
            send_id: "kid-note".into(),
        }]
    );
    drop(server);
    let mut replayed = open_recorded(
        &host,
        &source,
        Recording::Off,
        Some(&records),
        LiveHttp::default(),
    );
    for record in &records {
        replayed.apply(record).expect("every record replays");
    }
    assert_eq!(replayed.take_outputs(), outputs, "outputs replay exactly");
    replayed.close().expect("close");
}

/// A networked session records its network inputs (outbound transport
/// decisions, the late `error.communication`, and an inbound message with
/// its response status) and replays byte for byte without any network.
#[test]
fn a_recorded_network_session_replays_byte_for_byte_without_the_network() {
    let host = host();
    let server = TestServer::start(Reply::Status(500));
    let (mut http, _woken) = network(&[&server.url("/")], Duration::from_secs(10));
    let public = PublicUrl::new("http://agent.example/scxml").expect("public URL");
    http.public_url = Some(public.clone());
    // The denied send fails at once (internal queue); the accepted one
    // fails later (the server answers 500); then an inbound `hello` ends it.
    let source = scxml(&format!(
        r#"<datamodel><data id="n" expr="0"/></datamodel><state id="s0"><onentry><send id="late-note" event="notify" type="{BASICHTTP}" target="{}"/><send id="denied-note" event="denied" type="{BASICHTTP}" target="https://denied.example/"/></onentry><transition event="error.communication" cond="n == 0"><assign location="n" expr="1"/><log label="sync" expr="_event.type + ' ' + _event.sendid"/></transition><transition event="error.communication" cond="n == 1" target="s1"><log label="late" expr="_event.type + ' ' + _event.sendid"/></transition></state><state id="s1"><transition event="hello" target="done"><log label="hello" expr="JSON.stringify(_event.data) + ' ' + _event.origintype"/></transition></state><final id="done"/>"#,
        server.url("/hook")
    ));
    let options = scxml_snapshot::Options::default();
    let mut live = open_recorded(&host, &source, Recording::On, None, http);
    let mut snapshots = vec![live.snapshot(&options).expect("snapshot")];
    settle(&mut live);
    snapshots.push(live.snapshot(&options).expect("snapshot"));
    assert_eq!(live.configuration().expect("configuration"), ["s1"]);
    let receipt = live.receive_http(&basichttp_types::HttpRequest {
        method: "POST".into(),
        uri: "/scxml/sessions/root/basichttp".into(),
        headers: vec![basichttp_types::HttpHeader {
            name: "Content-Type".into(),
            value: "application/x-www-form-urlencoded".into(),
        }],
        body: b"_scxmleventname=hello&who=world".to_vec(),
    });
    assert_eq!(receipt.response.status, 200);
    let step = receipt
        .result
        .expect("the step succeeds")
        .expect("enqueued");
    assert_eq!(step.status, LiveStatus::Finished);
    snapshots.push(live.snapshot(&options).expect("snapshot"));
    let outputs = live.take_outputs();
    let texts: Vec<String> = logs(&outputs).into_iter().map(|(_, text)| text).collect();
    assert_eq!(
        texts,
        [
            "platform denied-note",
            "platform late-note",
            &format!(r#"{{"who":"world"}} {BASICHTTP}"#)
        ]
    );
    let records = live.take_recorded();
    live.close().expect("close");
    let outbound: Vec<bool> = records
        .iter()
        .filter_map(|record| match record {
            InputRecord::OutboundHttp { accepted } => Some(*accepted),
            _ => None,
        })
        .collect();
    assert_eq!(outbound, [true, false], "{records:?}");
    assert!(
        records.iter().any(|record| matches!(
            record,
            InputRecord::CommunicationErrors { senders, .. }
                if senders.len() == 1 && senders[0].send_id == "late-note"
        )),
        "{records:?}"
    );
    assert!(
        records.iter().any(|record| matches!(
            record,
            InputRecord::Http { session_id, status: 200, event, .. }
                if session_id == "root" && event.name.as_str() == "hello"
        )),
        "{records:?}"
    );

    // Replay: the same public URL, no transport, no listener, no server.
    drop(server);
    let mut replayed = open_recorded(
        &host,
        &source,
        Recording::Off,
        Some(&records),
        LiveHttp {
            public_url: Some(public),
            ..LiveHttp::default()
        },
    );
    let mut replayed_snapshots = Vec::new();
    for record in &records {
        match replayed.apply(record).expect("every record replays") {
            Applied::Snapshot(bytes) => replayed_snapshots.push(bytes),
            Applied::Call(_)
            | Applied::Received(_)
            | Applied::OutboundDecision
            | Applied::Started => {}
        }
    }
    assert_eq!(replayed.take_outputs(), outputs, "outputs replay exactly");
    assert_eq!(
        replayed_snapshots, snapshots,
        "snapshots replay byte for byte"
    );
    assert_eq!(replayed.status(), LiveStatus::Finished);
    replayed.close().expect("close");

    replays_diverge_on_another_lineage(&host, &source, &records);
}

/// A recorded message whose lineage is not its session's (here the root
/// with a non-empty path) reaches no session: the replay diverges.
fn replays_diverge_on_another_lineage(host: &SessionHost, source: &str, records: &[InputRecord]) {
    let doctored: Vec<InputRecord> = records
        .iter()
        .cloned()
        .map(|record| match record {
            InputRecord::Http {
                at_ms,
                session_id,
                event,
                status,
                ..
            } => InputRecord::Http {
                at_ms,
                session_id,
                path: vec!["ghost".into()],
                event,
                status,
            },
            other => other,
        })
        .collect();
    let mut diverged = open_recorded(
        host,
        source,
        Recording::Off,
        Some(&doctored),
        LiveHttp {
            public_url: Some(PublicUrl::new("http://agent.example/scxml").expect("public URL")),
            ..LiveHttp::default()
        },
    );
    let failure = doctored
        .iter()
        .find_map(|record| diverged.apply(record).err())
        .expect("the doctored lineage diverges");
    assert!(failure.to_string().contains("replay-diverged"), "{failure}");
}

#[test]
fn a_delayed_post_is_sent_when_due_and_replays_without_the_network() {
    // SCXML 6.2: "It MUST dispatch the message only when the delay interval
    // elapses." Nothing reaches the server before 50ms; the POST then fails
    // after acceptance (500), and the late error.communication carries the
    // send's id (5.10.1) on the internal queue (6.2.4). The recorded session
    // replays byte for byte without the network, pending send included.
    let host = host();
    let server = TestServer::start(Reply::Status(500));
    let (http, _woken) = network(&[&server.url("/")], Duration::from_secs(10));
    let source = scxml(&format!(
        r#"<state id="s0"><onentry><send id="later" event="notify" type="{BASICHTTP}" target="{}" delay="50ms"><param name="n" expr="1"/></send></onentry><transition event="error.communication" target="lost"><log label="late" expr="_event.type + ' ' + _event.sendid"/></transition></state><final id="lost"/>"#,
        server.url("/hook")
    ));
    let options = scxml_snapshot::Options::default();
    let mut live = open_recorded(&host, &source, Recording::On, None, http);
    let pending = live.snapshot(&options).expect("snapshot");
    let document: serde_json::Value = serde_json::from_slice(&pending).expect("JSON document");
    assert_eq!(
        document["snapshot"]["root"]["pending_sends"][0]["basichttp"]["event"], "notify",
        "the pending POST is in the snapshot"
    );
    let mut snapshots = vec![pending];
    live.advance_to(49).expect("advance");
    assert!(
        server
            .received
            .recv_timeout(Duration::from_millis(200))
            .is_err(),
        "nothing is posted before the delay elapses"
    );
    live.advance_to(50).expect("advance");
    let received = server
        .received
        .recv_timeout(Duration::from_secs(5))
        .expect("the POST is sent when due");
    assert_eq!(received.body, "_scxmleventname=notify&n=1");
    settle(&mut live);
    assert_eq!(live.configuration().expect("configuration"), ["lost"]);
    snapshots.push(live.snapshot(&options).expect("snapshot"));
    let outputs = live.take_outputs();
    assert_eq!(
        logs(&outputs),
        [("root".to_owned(), "platform later".to_owned())]
    );
    let records = live.take_recorded();
    live.close().expect("close");
    assert!(
        records
            .iter()
            .any(|record| matches!(record, InputRecord::OutboundHttp { accepted: true })),
        "{records:?}"
    );

    drop(server);
    let mut replayed = open_recorded(
        &host,
        &source,
        Recording::Off,
        Some(&records),
        LiveHttp::default(),
    );
    let mut replayed_snapshots = Vec::new();
    for record in &records {
        if let Applied::Snapshot(bytes) = replayed.apply(record).expect("every record replays") {
            replayed_snapshots.push(bytes);
        }
    }
    assert_eq!(replayed.take_outputs(), outputs, "outputs replay exactly");
    assert_eq!(
        replayed_snapshots, snapshots,
        "snapshots replay byte for byte"
    );
    replayed.close().expect("close");
}

#[test]
fn a_cancelled_delayed_post_never_reaches_the_server() {
    // SCXML 6.3: "The <cancel> element is used to cancel a delayed <send>
    // event."
    let host = host();
    let server = TestServer::start(Reply::Status(200));
    let (http, _woken) = network(&[&server.url("/")], Duration::from_secs(10));
    let source = scxml(&format!(
        r#"<state id="s0"><onentry><send id="later" event="notify" type="{BASICHTTP}" target="{}" delay="50ms"/><send event="stop" delay="10ms"/></onentry><transition event="stop" target="s1"><cancel sendid="later"/></transition></state><state id="s1"><transition event="error.communication" target="lost"/></state><final id="lost"/>"#,
        server.url("/hook")
    ));
    let mut live = open(&host, &source, Vec::new(), http);
    live.advance_to(100).expect("advance");
    settle(&mut live);
    assert!(
        server
            .received
            .recv_timeout(Duration::from_millis(300))
            .is_err(),
        "a cancelled POST is never sent"
    );
    assert_eq!(live.configuration().expect("configuration"), ["s1"]);
    live.close().expect("close");
}

/// A replay that runs out of recorded outbound decisions fails closed
/// instead of guessing an answer.
#[test]
fn a_replay_without_its_outbound_decisions_diverges() {
    let host = host();
    let source = send_to("https://api.example/hook");
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        "network.scxml",
        source.as_str(),
    ))
    .expect("fixture parses");
    let ir = scxml_compiler_core::lower(&document).expect("fixture lowers");
    let error = host
        .open(SessionOpen {
            machine_ir: &ir.to_json().expect("fixture IR encodes"),
            data_model: ir.data_model.as_deref(),
            session_id: "root",
            embedder: Rc::new(Documents(Vec::new())),
            max_commands: 1_024,
            clock: LiveClock::Logical,
            record: false,
            replay: Some(&[]),
            http: LiveHttp::default(),
        })
        .expect_err("a replay without its decisions diverges");
    assert!(
        matches!(
            error.failure,
            scxml_host_wasmtime::RunFailure::Harness {
                code: "replay-diverged",
                ..
            }
        ),
        "{error:?}"
    );
}

/// `stop` returns promptly whatever address the listener is bound to: the
/// accept loop polls its stop flag, so nothing has to connect to wake it
/// (an unspecified address, `0.0.0.0` or `::`, is not one a client can
/// reach everywhere).
#[test]
fn a_listener_on_every_interface_stops_promptly() {
    for addr in ["0.0.0.0:0", "[::]:0"] {
        let listener = match BasicHttpListener::bind(
            addr.parse().expect("address"),
            ListenerConfig::default(),
        ) {
            Ok(listener) => listener,
            // A host without IPv6 cannot bind `::`.
            Err(_) if addr.starts_with('[') => continue,
            Err(error) => panic!("bind {addr}: {error}"),
        };
        let handle = listener.serve(|_| {}).expect("serve");
        let (stopped, done) = mpsc::channel();
        std::thread::spawn(move || {
            handle.stop();
            let _ = stopped.send(());
        });
        done.recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|_| panic!("stop on {addr} returns"));
    }
}

/// Dropping a handle stops its listener: by the time the drop returns, the
/// accept thread has ended and released everything it owned, the bound
/// socket and the `deliver` callback (whose captured probe is then the
/// only other owner of `alive`). No port is probed, so another test that
/// binds the freed port cannot interfere.
#[test]
fn dropping_a_listener_handle_stops_it() {
    let listener = BasicHttpListener::bind(
        "127.0.0.1:0".parse().expect("address"),
        ListenerConfig::default(),
    )
    .expect("bind the listener");
    let alive = std::sync::Arc::new(());
    let probe = std::sync::Arc::clone(&alive);
    let handle = listener
        .serve(move |_| {
            let _ = &probe;
        })
        .expect("serve");
    assert_eq!(
        std::sync::Arc::strong_count(&alive),
        2,
        "the accept thread holds the callback"
    );
    drop(handle);
    assert_eq!(
        std::sync::Arc::strong_count(&alive),
        1,
        "the accept thread ended and dropped the callback and the socket"
    );
}
