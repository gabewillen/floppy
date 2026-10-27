# Event I/O Processors

An Event I/O Processor (SCXML 6.1, 6.2) carries a `<send>` of its type to
the outside world and turns what comes back into external events. The SCXML
and BasicHTTP processors are built in. Any other processor is a Wasm
component of the `stateforward:scxml/event-processor` world that a package
binds to a type URI: the engine hands every `<send>` whose type is neither
SCXML nor BasicHTTP to the host (`processor-send`), and the Wasmtime session
host sends it to the component bound to that type. A type no package binds
is `invalid-input`, which the document sees as `error.execution`.

The repository ships one such component, the WebSocket processor
(`components/scxml-ioprocessor-websocket`). Nothing in the host or the
engine names it: any component of the world binds the same way.

## The component contract

The `event-processor` world exports `event-processor-api`, and may import
`outbound-http`, `outbound-websocket`, and `telemetry` (a component imports
only what it uses).

- **`open`**: the host instantiates the component once per session, on that
  session's first `<send>` of its type, in a store of its own, and calls
  `open` once. It returns the stream of events the instance delivers to the
  session, and a future that resolves when the instance can send no more
  (`some` error raises one `error.communication` without a send id).
- **`send`**: one call per `<send>` (`delay-ms` is 0). A call that resolves
  before the instance is idle decides the `<send>`: `ok` accepts it,
  `invalid-input` and `payload-too-large` raise `error.execution`, anything
  else `error.communication` with the send id. A call still pending accepts
  it, and an error it resolves with later raises `error.communication` with
  the send id.
- **`check`**: a delayed `<send>` is checked when it is evaluated, held on
  the session clock by the SCXML processor (`schedule-processor`, so
  `<cancel>` cancels it), and sent with `send` when it falls due.
- **Events**: the host sets each event's type to `external` and its
  `origintype` to the processor's type URI; the processor sets `origin`.
- **End**: when the session ends the host cancels every call and drops the
  store, which closes the instance's connections.

Each call runs under the host's per-input fuel. Open connections do not hold
a session's logical clock; a request a processor has in flight does.

## Packages

A `stateforward.scxml.processor-package/v1` manifest binds one component:

```json
{
  "format": "stateforward.scxml.processor-package/v1",
  "processor-uri": "https://github.com/stateforward/scxml.rs/processors/websocket",
  "aliases": ["websocket"],
  "package-version": "0.2.0",
  "processor-artifact": "scxml_ioprocessor_websocket.virtualized.wasm",
  "wit-world": "stateforward:scxml/event-processor",
  "wit-version": "0.2.0",
  "origins": ["https://api.example.com"]
}
```

A `<send type>` names the URI or one of its aliases. `origins` lists the
`http://` or `https://` origins (or path prefixes) the component may reach;
binding the package consents to them. A WebSocket URL is admitted by its
HTTP form: `wss://api.example.com/v1/stream` by `https://api.example.com`.
The checked-in manifest lists no origins, so a deployment writes its own.

## The WebSocket processor

`<send type="websocket" target="wss://...">` sends one message over a
connection to its target URL. Connections are per target URL and per
session: the first `<send>` to a URL opens one, and it stays open until the
peer closes it, it is lost, the document closes it, or the session ends.

| `<send>` | Message |
| --- | --- |
| `<content>` | the content: a string as text, any other value as its JSON text |
| only the parameter `_scxmlbinary` | its base64 value, decoded, as a binary message |
| anything else | a JSON object text: `_scxmleventname` (the event name, when there is one) and each parameter as a member |
| `event="websocket.close"`, no parameters, no content | no message: the connection starts closing (code 1000) |

Numbers are written as `JSON.stringify` writes them (`1`, not `1.0`). What
the peer sends becomes an event whose `origin` is the target URL:

| Message | Event |
| --- | --- |
| a JSON object with a string `_scxmleventname` | that event; its other members are the data |
| any other text | `websocket.message`; its JSON value, or the text itself |
| binary | `websocket.message` with `{"_scxmlbinary": <base64>}` |
| the connection's end | `websocket.close` with `{"code", "reason"}`: the peer's close frame, or 1006 when it was lost |

A `<send>` resolves once its message is handed to the connection. One whose
connection cannot be opened, or that was still queued when the connection
ended, raises `error.communication`; one that cannot be mapped raises
`error.execution`. After a close, the peer may still send messages until its
own close frame arrives, and they still reach the session; the next `<send>`
to that URL opens a new connection. Closing a URL with no connection does
nothing. Each message is at most 1 MiB either way.

```xml
<state id="listening">
  <onentry>
    <send type="websocket" target="wss://api.example.com/v1/stream" event="hello"/>
  </onentry>
  <transition event="websocket.message">
    <log label="heard" expr="_event.data"/>
  </transition>
  <transition event="done">
    <send type="websocket" targetexpr="'wss://api.example.com/v1/stream'" event="websocket.close"/>
  </transition>
  <transition event="websocket.close" target="idle"/>
</state>
```

## `scx run`

`run` binds processors with these flags (other commands do not take them):

- `--processor-package MANIFEST.json` (repeatable) binds one package.
- `--processor-component URI=PATH` names the component; by default it is
  `processor-artifact` beside the manifest.
- `--processor-component-sha256 URI=HEX` authenticates the component; it is
  required for every package, because the manifest is not a trust anchor.
- `--websocket-proxy http://HOST:PORT` tunnels every WebSocket connection
  through that HTTP proxy with `CONNECT`. The environment is never read for
  a proxy.

`run` lists each binding (type URI, aliases, version, origins) on stderr.
The log header records the bindings, and a replay must bind the same
components. A run with an open connection does not end at the end of its
input. Each answer a connection delivers (opened, a message, closed, a
failure) is a `websocket` record of the replay log, so a replay needs no
network.

## Host API

```rust
let processors = ProcessorComponents::new()
    .with_outbound_websocket(Arc::new(WebSocketClient::new(&WebSocketClientConfig::default())?))
    .bind(package, &authenticate_processor_component(&bytes, digest)?)?;
let host = SessionHost::new(&components, &config, &providers)?
    .with_processor_components(processors)?;
```

`WebSocketClient` (the `http` feature) is a `WebSocketTransport`: `ws://` and
`wss://` over rustls, verified against the operating system's trust store,
with an optional `CONNECT` proxy. An embedder may supply its own transport.
A live session records each answer and replays it without a transport.

## Telemetry

The Wasmtime host adds the boundary spans
`scxml.host-wasmtime.processor-open` (per instance opened),
`processor-send` (per `<send>` handed to an instance), and
`processor-websocket-connect` (per connection admitted), and
`processor-websocket` (per connection, from its handshake to its end, on its
thread), which an invoked service's connections
([invoked-services.md](invoked-services.md#outbound-websocket)) record too;
the CLI adds `scxml.cli.read-processor-package`. Each records only
its closed outcome, signal event, and error category: never a type URI,
URL, header, or message.

## Limits

- Only the Wasmtime session host and `scx run` bind processor packages; the
  browser host and batch runs (`validate`, `compile`, `snapshot run`, the
  IRP) refuse any type but SCXML and BasicHTTP.
- `_ioprocessors` lists no bound processor, so a document cannot read a
  processor's location from it.
- A document cannot bind a processor itself; the embedder binds packages.
- A document closes a WebSocket connection with code 1000 only; it cannot
  choose the code or reason.
- A session snapshot does not include processor instances or their
  connections.
