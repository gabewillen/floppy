# Live sessions

A live session keeps one root SCXML session (and the sessions it invokes)
alive between calls. Each external event is processed to a stable configuration when it
arrives, and delayed `<send>`s fire when their deadline comes due. Two
surfaces expose it: the Wasmtime host API (`SessionHost::open`, in
`hosts/scxml-host-wasmtime`) and the `scx run` command. Both execute
only through the shipped components, like every other run.

Nothing here adds to SCXML. A document behaves exactly as SCXML 1.0 and its
Event I/O Processors define. Clocks, command budgets, outputs, record and replay,
and the command line are host concerns, and documents cannot observe them.

## Host API

```rust
use std::rc::Rc;
use scxml_host_wasmtime::{
    LiveClock, LiveHttp, ScriptedEvent, SessionHost, SessionOpen, SessionOutput, WallClock,
};

let host = SessionHost::from_release_dir(&release_dir, &manifest_sha256, fuel)?;
let mut session = host.open(SessionOpen {
    machine_ir: &ir,                  // lowered Machine IR
    data_model: Some("ecmascript"),
    session_id: "session",
    embedder: Rc::new(my_documents),  // SessionEmbedder: <invoke src>, scripts
    max_commands: u64::MAX,           // engine-command budget of each call (none)
    clock: LiveClock::Timed(Box::new(WallClock::start())),
    record: true,                     // keep an input log for replay
    replay: None,                     // or Some(&records) to replay a log
    http: LiveHttp::default(),        // no BasicHTTP network (see below)
})?;
session.send(&ScriptedEvent::named("go"))?;    // process one event
while let Some(deadline) = session.next_deadline()? {
    // wait until session.clock_ms() reaches `deadline` (or the next input)
    session.advance()?;                         // fire what is due
}
for output in session.take_outputs() { /* Log, Done */ }
let document = session.snapshot(&Default::default())?;  // between calls
let report = session.close()?;                 // exit, cancel, shut down
```

`SessionHost::open` constructs and starts the root, then runs every session
until nothing is runnable at session time 0.

### Calls

Every public call has its own engine-command budget (`max_commands`).
Each machine call inside it keeps the shared per-call fuel budget. A call
ends with every session in a stable configuration.

- `send` validates the event exactly as a batch run validates scripted
  events. It fires the deadlines the clock has already reached, in deadline
  order, then puts the event on the root's external event queue and runs every
  session until nothing is runnable at the current time. A call that fails
  moves the session to `Failed`, and after that only `close` works. Some
  failures leave the session unchanged instead: an invalid event name
  (`event-name-invalid`), an event after the root finished
  (`event-after-finish`), or an event the machine rejects.
- `advance` fires what is due. A timed session moves to its clock's reading
  and fires every deadline at or before it. A logical session moves to its
  next deadline. If nothing is due, `advance` only moves the clock.
- `advance_to(ms)` and `run_until_idle()` are for logical sessions only.
  `run_until_idle` fires every pending deadline in order, which is the batch
  run's drain, except that it never moves the clock past an invoked
  service's request still in flight (`awaiting_http_responses`): it
  returns, and the embedder waits for the response (see "Invoked services'
  HTTP responses" below) before it calls it again.
- `snapshot(options)` returns the canonical `scxml.session-snapshot/v2`
  document of the session in its stable configuration ([session-snapshot.md](session-snapshot.md)).
  For the same state it is the same document a batch run produces:
  `hosts/scxml-host-wasmtime/tests/session_snapshot.rs` replays every fixture
  of that harness through a live session and compares the bytes.
- `close` ends the session gracefully. A running root exits: its active
  configuration's `<onexit>` runs and every invocation is cancelled. Pending
  delayed sends are cancelled, and every session is dropped, so each
  session's extension components shut down exactly once. The report
  counts the cancelled sends and children and carries the exit's outputs.
  Dropping a session without `close` shuts it down the same way but skips
  the root's exit.

`SessionHost::run` (batch) drives the same engine through the same stepwise
scheduler calls. It starts the root, delivers every scripted event, then
drains with the logical clock unbounded, and it behaves byte for byte as
before.

### Time

Session time is a logical millisecond count. It is 0 at the moment the root
starts, after construction and any first-use component compilation. It never
moves backwards: a request to move it back fails with `clock-regression`.

- `LiveClock::Timed(clock)` reads a `SessionClock` at every call. There are two:
  - `WallClock` is the process's monotonic clock.
  - `ManualClock` is set by hand, for deterministic tests.

  An event is processed at the time it arrives, and `advance` fires what the
  clock has reached.
- `LiveClock::Logical` moves only when the embedder asks. Replay uses it.

The host starts no thread and runs nothing between calls. The embedder owns
the loop: it waits until `next_deadline` (compared with `clock_ms`) or the
next input, then calls `advance` or `send`. So the single-threaded (`!Send`)
session and its stores never cross a thread, while the compiled-component
cache stays process-wide.

### Outputs

`take_outputs` returns, in the order the sessions produced them:

- `Log { session, parent, entry }`: every `<log>` of every session.
- `Done`: the root reached a top-level final state (once). The root's
  `<donedata>` is not reported, because the machine returns it only to an
  invoking session.

`io_processors()` returns the root's `_ioprocessors` locations: each linked
processor's type URI and aliases, with the session's address.

### BasicHTTP

A live session speaks the BasicHTTP Event I/O Processor (SCXML 1.0 appendix
C.2) through the shipped `scxml-ioprocessor-basichttp` component, which
encodes every request, decodes every inbound message, and chooses every
response status. The host only moves bytes. `SessionOpen::http` (`LiveHttp`)
decides where they go:

- `public_url` (a `PublicUrl`): every session's location, the root's and
  every invoked session's at any depth, is
  `{public_url}/sessions/{_sessionid}/basichttp`, and `_ioprocessors`
  advertises it. Without one, locations stay under the unresolvable
  `http://basichttp.loopback.invalid` authority, as in a batch run.
- A POST to the sender itself, its parent, or one of its live children is
  delivered in-process at once (the loopback, unchanged). Any other location
  in the session's own address space is unknown (`error.communication`).
- `outbound` (an `OutboundHttp`): every POST outside the address space goes
  to it. It must return at once. Accepting a request tells the component
  `202 Accepted`, so the `<send>` succeeds; refusing it (a denied target, no
  capacity) makes the send `error.communication` at once. Without a
  transport every such POST is refused.
- An accepted request settles its `CommunicationFailure` from any thread.
  A failure (no response, or a non-2xx status) owes the sender one
  `error.communication`. `on_communication_error` wakes the embedder, which
  calls `deliver_communication_errors`: at the session's current time (due
  deadlines fire first), each owed event goes on its sender's internal
  event queue when that very session still runs (SCXML 3.12.2: the
  processor "MUST place these events in the internal event queue"; 6.2.4:
  "If it is unable to dispatch the message, the Processor MUST place the
  error error.communication on the internal event queue"), and every
  session runs to a stable configuration. The platform raised the event, so its
  `_event.type` is `platform` and its `_event.sendid` is the failed
  `<send>`'s id (SCXML 5.10.1: the authored `id`, else the one the machine
  generated for that execution), with no data, as the browser host
  delivers a late fetch failure. `communication_pending` says whether an
  accepted request may still fail.

  Appendix D mapping: `mainEventLoop` runs a macrostep (eventless
  transitions, then the internal queue until it is empty) and only then
  blocks on the external queue. A late error arrives while its session
  waits there, after the macrostep of its `<send>` completed. The machine's
  `communication-error` places it on the internal queue and runs the
  macrostep that drains it at once, dequeuing no external event, so it is
  never autoforwarded (SCXML 6.4.1 forwards external events) and never
  waits behind queued external events. A delayed send whose target no
  longer answers when it falls due takes the same path in batch and live
  runs alike.
- Invoked services' HTTP responses: an invoked service's
  `outbound-http.fetch` is an `async func` that suspends only the calling
  task ([invoked-services.md](invoked-services.md#outbound-http)). A live
  session performs each admitted request through the transport
  `ServiceComponents::with_outbound_http` grants, on a transport thread of
  its own, and keeps taking events and firing timers meanwhile.
  `LiveHttp::on_http_response` is called from any thread when a response
  arrives, so the embedder calls `deliver_http_responses`: at the session's
  current time (due deadlines fire first), each response reaches the
  invocation that waits for it, whose event loop runs until it waits again,
  and every session runs to a stable configuration. A response of an
  invocation that was cancelled (its state exited) or ended meanwhile is
  dropped, and cancelling it closed that request's connection, so its
  thread ended instead of waiting. A recording session records each
  delivered response (`HttpResponse`); a replaying session performs no
  request and delivers each response from the record.
- `receive_http(request)` delivers one inbound message. Its target, in
  origin form or absolute, must end in `/sessions/{_sessionid}/basichttp`
  (so a proxy may mount the listener under a prefix). The message is queued
  on the external queue of the running session with that id, at the
  session's current time, and every session runs to a stable configuration, like `send`.
  The returned `HttpReceipt` holds the component's response (200 queued,
  400/405/413 rejected, 404 no running session there, 503 queue full, or a
  bare 500 when the host failed before the component answered) and the call result.
  `http_rejection` answers a message a server could not frame.

The `http` feature adds both halves. `NetworkClient` is an `OutboundHttp`
with a deny-by-default `HttpAllowlist` of origins and URL prefixes. Every
target is canonicalized with the WHATWG URL parser before the check, so
`..`, `%2e%2e`, and backslash segments cannot leave an admitted prefix, and
the canonical URL is the one requested. Userinfo is refused. No cookies,
credentials, environment proxy, or redirects are used. Connect and total
timeouts, an in-flight bound, and a response-head bound apply, and response
bodies are never read. `BasicHttpListener` is a minimal HTTP/1.1 server: one
request per connection, `Content-Length` framing only (`Transfer-Encoding`
is rejected as malformed), bounded connections, head, headers, and body, and
read, write, and answer timeouts. Each message reaches the session thread as
an `InboundHttp`, and its `respond` sends the component's answer back.

Record and replay cover the network (see Record and replay below).

## Record and replay

A session opened with `record: true` logs every input as an `InputRecord` at
the session time it was applied:

- `Start { epoch_ms }`: always the first record, the Unix epoch ms at which
  the session clock's logical time 0 is (what ECMAScript `Date` reads is
  this plus the logical time; a timed session starts at the wall-clock time
  it opened, and a logical one at `BATCH_START_EPOCH_MS`,
  2015-09-01T00:00:00Z); a replay starts its clock there
- `Event { at_ms, event }`
- `Advance { to_ms }`
- `Idle` (a `run_until_idle`)
- `Snapshot { options }`
- `Http { at_ms, session_id, path, event, status }`: an inbound message
  that reached a running session, as the component decoded it, with the
  response status it chose; `path` is the addressed session's lineage (the
  invoke ids from the root to it, empty for the root). Replay addresses the
  session by `session_id` and delivers only to a session of that lineage;
  any other session there, or none, diverges
- `CommunicationErrors { at_ms, senders }`: one delivery of late
  `error.communication` events, one per failed send (its session and send
  id) whose sending session was still in the run when the record was
  written; an event owed to a session that has left the run reaches no
  one, so it is not recorded
- `OutboundHttp { accepted }`: one outbound request's transport decision,
  in request order (a refusal raises `error.communication` inside the
  `<send>`, so the decision is an input); the requests themselves are
  outputs and are not recorded
- `HttpResponse { at_ms, path, request, response }`: the response (or the
  transport's failure) to an invoked service's `outbound-http.fetch`;
  `path` is the invocation's lineage of invoke ids and `request` the
  request's number within the invocation, from 0. Replay delivers it from
  the record, only to the invocation of that lineage; any other diverges

`take_recorded` drains the log. The engine is deterministic given its calls
and their times. So opening a `LiveClock::Logical` session over the same
Machine IR, components, namespace bindings, fuel, and `max_commands`, then
calling `apply` on each record in order, makes exactly the same engine calls
at exactly the same times. It reproduces every output and every snapshot
byte for byte. A networked session replays without the network: open it
with the records as `SessionOpen::replay` (which answers every outbound
request from the recorded decisions, in order), the recorded public URL,
and no transport; a replay that runs out of decisions fails with
`replay-diverged`, as does an inbound message the component answers with
another status. `tests/live_session.rs` checks this for a timed session
driven by a `ManualClock`, `tests/basichttp_network.rs` for a session with
an inbound message, a denied send, and a late failure, and
`crates/scxml-cli/tests/live_run.rs` for wall-clock CLI runs, one of them
over real sockets.

Replay does not capture what a document reads from outside its inputs:
documents loaded through `<invoke src>` or `<data src>`, the environment
passed to namespace extension components, and what an invoked service reads
through its WASI resources. Replay must supply the same ones.

## The `run` command

```console
cargo run --release -p scxml-cli --features wasmtime -- run machine.scx \
  --record session.ndjson
{"name":"go","data":{"order":42}}
{"type":"snapshot"}
```

`scx run DOC` compiles DOC as `snapshot run` does and opens one live
session. It reads newline-delimited JSON inputs on standard input and writes
one JSON record per line on standard output. Standard error carries
diagnostics only, in rustc style (`error[code]: …`, `warning[code]: stdin
line N: …`).

It takes the same component, `--extension-package`,
`--extension-component`, `--extension-component-sha256`, `--env`,
`--env-file`, and repository namespace (`--offline`, `--cache-dir`,
`--github-*`) flags as `snapshot run`, plus these:

| Flag | Meaning |
| --- | --- |
| `--fuel N` | store fuel of every host call (default: the host's `DEFAULT_FUEL`) |
| `--max-commands N` | engine-command budget of each call (default: none) |
| `--session-id NAME` | the root `_sessionid` (default `session`) |
| `--clock wall\|logical` | `wall` (the default) fires delayed sends at their real deadline; `logical` fires them all right after each event |
| `--snapshot-on call\|done\|never` | write a snapshot after every call, once when the root finishes, or never (the default) |
| `--exclude SECTION`, `--trace-context strip\|preserve` | options of every snapshot, as for `snapshot run` |
| `--on-eof wait\|close` | at the end of input, keep firing delayed sends until the root finishes or nothing is scheduled (`wait`, the default), or close at once |
| `--record FILE` | write the replay log |
| `--replay FILE` | reproduce a recorded session on the logical clock instead of reading standard input |
| `--http-allow ORIGIN_OR_PREFIX` | let BasicHTTP POSTs reach this origin (`https://api.example`) or URL prefix (`https://api.example/hooks`); repeatable; none by default, so every POST outside the session's own locations raises `error.communication` |
| `--http-listen PORT\|IP:PORT` | serve every session's BasicHTTP location (`PORT` binds 127.0.0.1); the session then stays open after the end of input |
| `--http-public-url URL` | the base every location is advertised under (default `http://ADDR` of the listener), for a listener behind a proxy or tunnel; required when binding `0.0.0.0` or `::` |

`--record` records the network inputs, and `--replay` reproduces them
without the network, so the http flags cannot be given with `--replay` (see
Replay log below). An inbound
message runs as a call with cause `http`, and the owed `error.communication`
of accepted requests that failed later runs as a call with cause
`communication-error`. Two processes talk to each other like this (A
answers every ping at the reply address it carries):

```console
scx run pong.scx \
  --session-id a --http-listen 8401 --http-allow http://127.0.0.1:8402 < /dev/null &
scx run ping.scx \
  --session-id b --http-listen 8402 --http-allow http://127.0.0.1:8401 < /dev/null
{"type":"opened","session":"b","clock":"wall","ioprocessors":{"basichttp":{"location":"http://127.0.0.1:8402/sessions/b/basichttp"},…}}
{"type":"call","cause":"start","at_ms":0,"status":"running","configuration":["waiting"],"next_deadline_ms":null,"commands":0}
{"type":"log","session":"b","label":"pong","text":"7"}
{"type":"done"}
{"type":"call","cause":"http","at_ms":252,"status":"finished","configuration":["done"],"next_deadline_ms":null,"commands":1}
{"type":"closed","reason":"done","status":"finished","cancelled_sends":0,"cancelled_children":0}
```

where `ping.scx` sends
`<send event="ping" type="basichttp" target="http://127.0.0.1:8401/sessions/a/basichttp">`
with a `reply` parameter of `_ioprocessors['basichttp'].location`, and
`pong.scx` answers with
`<send event="pong" type="basichttp" targetexpr="_event.data.reply">`
(the full documents are in
[cli.md](cli.md#basichttp-between-two-processes)). Any HTTP client can
post to a listening session, for example
`curl --data-urlencode _scxmleventname=ping http://127.0.0.1:8401/sessions/a/basichttp`.

### Inputs (standard input)

Each input is one JSON object on one line:

- `{"name": EVENT, "data": JSON}` or `{"type": "event", …}`: an external event.
  `data` is optional. Objects and arrays become canonical JSON values. Use
  `"xml": "<…>"` instead of `data` for an XML payload.
- `{"type": "snapshot"}`: write a snapshot record now.
- `{"type": "close"}`: close the session.

An input that does not parse, has an unknown field, or is not UTF-8 gets a
non-fatal `error` record carrying its line number, and the session goes on.

### Records (standard output)

| `type` | Fields |
| --- | --- |
| `opened` | `session`, `clock`, `ioprocessors` (`{uri: {"location": …}}`) |
| `log` | `session`, `parent` (children only), `label` (when present), `text` |
| `call` | `cause` (`start`, `event`, `timer`, `idle`, `http`, `communication-error`, `http-response`), `at_ms`, `status`, `configuration`, `next_deadline_ms`, `commands` |
| `snapshot` | `document`: the canonical snapshot document, embedded byte for byte |
| `done` | the root reached a top-level final state; when that final state has `<donedata>`, its evaluated value as `data` (JSON kinds) or `xml` (SCXML 5.5: the processor "SHOULD return the data to the environment in an implementation-dependent manner") |
| `error` | `code`, `message`, `fatal`, `line` (input errors) |
| `closed` | `reason` (`done`, `eof`, `close`, `signal`, `failed`, `replayed`), `status`, `cancelled_sends`, `cancelled_children` |

The outputs of a call come before its `call` record.

### Exit status

| Status | Meaning |
| --- | --- |
| 0 | the root reached a top-level final state |
| 1 | the session failed (a fatal `error` record, then `closed` with reason `failed`) |
| 2 | a usage, compilation, component, replay-log, or I/O failure before or around the session |
| 3 | the session closed while running: end of input, a `close` input, or the end of a replay |
| 128 + N | signal N (`SIGINT` 130, `SIGTERM` 143) closed the session gracefully |

Signals are handled on Unix. A watcher thread forwards them as messages to
the single-threaded session loop. Elsewhere the platform default applies.

### Replay log

`--record FILE` writes NDJSON. The first line is a header:

```json
{"type":"header","format":"scxml.live-log/v1","ir_sha256":"…","components_manifest_sha256":"…","session_id":"session","max_commands":18446744073709551615,"fuel":…,"clock":"wall","namespaces":[{"uri":"…","extension_component_sha256":"…"}],"services":[{"uri":"…","service_component_sha256":"…"}],"http_public_url":null}
```

`namespaces` and `services` list each bound extension and invoked service
by URI with the SHA-256 its component is authenticated under, sorted by
URI. `http_public_url` is the advertised BasicHTTP base of a `--http-listen`
run (else `null`). One line per input follows: `start` (`epoch_ms`, always
first), `event` (`at_ms`, `name`,
`data`/`xml`), `advance` (`to_ms`), `idle`, `snapshot` (`options`), `http`
(`at_ms`, `session`, its invoke-id `path` from the root, `status`, and the
decoded `event`: `name`, optional
`sendid`, `origin`, `origintype`, `data`/`xml`, `raw`, and
`trace_context`), `communication-error` (`at_ms`, `senders`: each
`session`, invoke-id `path`, and the failed send's `sendid`), `outbound`
(`accepted`), or `http-response` (`at_ms`, the invocation's invoke-id
`path`, `request`, and either `status` with `body` (UTF-8) or `body_hex`,
or `failure`: `not-allowed`, `transport`, or `response-too-large`). A final
`{"type":"close","reason":…}` line marks the end. The log holds every
input's payload, including each inbound message's data, raw text, and W3C
carrier as received, and each service response's body, so it needs the
same access and retention protection as those messages.

`--replay FILE` checks the header against the document's Machine IR, the
component release, the namespace bindings, and the service bindings (a
different service component digest, or a service bound or unbound). Any
mismatch fails with
`[replay-identity-mismatch]`. The session id, fuel, and command budget come
from the log, so `--replay` refuses every flag that would change them. So
does the network: a replay advertises the recorded `http_public_url`,
answers outbound requests from the `outbound` records, and never listens or
connects, so it refuses the `--http-*` flags. The replayed standard output
equals the live run's, record for record, except the `opened` record's
`clock` (always `logical`).

## Telemetry

Each call has one boundary span with a closed operation label. Every one runs
under the session's single `session-run` span:

| Label | Covers |
| --- | --- |
| `scxml.host-wasmtime.session-open` | construct, start, and run the root to a stable configuration |
| `scxml.host-wasmtime.session-send` | one event call |
| `scxml.host-wasmtime.session-advance` | an advance that fires something, or a drain to idle |
| `scxml.host-wasmtime.session-receive-http` | one inbound BasicHTTP message that reached a session's queue |
| `scxml.host-wasmtime.session-communication-error` | one delivery of owed `error.communication` events |
| `scxml.host-wasmtime.session-http-response` | one delivery of an invoked service's HTTP response |
| `scxml.host-wasmtime.session-close` | the graceful close |
| `scxml.host-wasmtime.basichttp-network-post` | one outbound request (`http` feature), on its own thread, a child of the send that issued it |
| `scxml.host-wasmtime.basichttp-listener-request` | one inbound connection of the built-in listener (`http` feature) |
| `scxml.host-wasmtime.basichttp-listener-accept` | one failed `accept` of the built-in listener (`http` feature): `input-read` for a connection that failed before it was accepted, `platform` for a listener failure, after which the listener waits (10 ms, doubling to 1 s) before accepting again |
| `scxml.host-wasmtime.basichttp-listener-connection` | one connection the built-in listener accepted but could not serve or keep as configured (`http` feature): `platform` with the `input-rejected` event for a connection dropped unanswered (a socket mode or timeout it could not set, a thread it could not start); `platform` with no event for a connection served on a thread the listener could not keep (a poisoned thread list), which stopping the listener does not wait for |
| `scxml.host-wasmtime.session-snapshot` | one snapshot (unchanged) |
| `scxml.cli.run` | one `scx run` |

An advance that fires nothing emits no span, so waiting costs no telemetry.
No attribute carries a session id, event name, target, URL, host, header,
HTTP status, time, or payload.
Commands and fuel stay measurements. None of this adds a metric dimension or
span attribute, so the cardinality table in
[CONTRIBUTING.md](../CONTRIBUTING.md) only gains these fixed operation names.

## Limits

- Invocation is the batch run's: invoked sessions invoke in turn, within
  the embedder's `InvokeLimits` (`SessionHost::with_invoke_limits`:
  `max_invoke_depth`, `max_invoked_sessions`; none by default); an
  invocation beyond them fails as that `<invoke>`'s `error.execution`.
- A live session opens a network connection only through a configured
  `LiveHttp` (see BasicHTTP above); outbound requests carry no W3C trace
  headers, and the built-in listener
  speaks plain HTTP without authentication, so expose it only behind a
  trusted proxy or tunnel ([SECURITY.md](../SECURITY.md)).
- The CLI accepts no W3C trace carrier on its inputs. `ScriptedEvent` takes
  one in the host API.
