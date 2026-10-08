# Security policy

## Supported versions

This project is pre-1.0 and experimental. Only the latest commit on the `main`
branch receives security fixes. There are no maintained release branches, and
no crate from this workspace is published to crates.io.

| Version | Supported |
| --- | --- |
| `main` (latest, `0.2.x`) | Yes |
| Any older commit or fork | No |

## Reporting a vulnerability

Please **do not** report security vulnerabilities through public GitHub
issues, discussions, or pull requests.

Report them privately through GitHub's private vulnerability reporting for this
repository:

1. Open <https://github.com/stateforward/scxml.rs/security/advisories/new>
   (the **Security** tab, then **Report a vulnerability**).
2. Describe the issue, the affected component or crate, and the commit you
   tested.
3. Include a minimal reproduction (for example an SCXML document, snapshot document,
   Machine IR file, or Wasm input) and the observed versus expected behavior.

If you cannot use GitHub's private reporting, email the maintainer at
<gabewillen@gmail.com> with the same information instead.

Maintainers will acknowledge a report as soon as practical and agree on a
disclosure timeline with the reporter. Fixes land on `main` and are announced
through a GitHub Security Advisory once a fix is available.

## Scope

This project is designed to process untrusted input. The following are in
scope:

- untrusted SCXML, XML, and XSD documents given to the validator, compiler, or
  CLI, including bypasses of documented size, depth, and work bounds (Unicode
  bidirectional controls that could hide what a document runs are linted by
  `LINT016` and escaped as `\u{XXXX}` in rendered diagnostics);
- untrusted extension package manifests and canonical `scxml.report/v3` report
  bytes given to decoders and reporters;
- untrusted lowered Machine IR and `scxml.session-snapshot/v2` documents given
  to decode paths (snapshot documents hold data-model values and, when
  preserved, trace carriers, so they get the protections of canonical
  validator artifacts and are never logged or indexed);
- untrusted Wasm component bytes and artifact manifests given to the Wasmtime
  host, including SHA-256 identity verification and fuel or memory limit
  bypasses;
- escapes from the deny-by-default capability surface of the Wasm components
  (for example a guest reaching filesystem, network, clock, or random
  capabilities it was not granted);
- ECMAScript data-model expressions that break the documented structural
  bounds;
- the Wasmtime host's BasicHTTP network of live sessions (the `http`
  feature, and `scxml run --http-allow`/`--http-listen`/`--http-public-url`),
  which opens real sockets only when configured: outbound, bypasses of the
  deny-by-default `--http-allow` allowlist (including URL forms whose WHATWG
  canonical form reaches a different origin or path than the one the
  allowlist judged: dot segments, `%2e%2e`, backslashes, userinfo, or
  default ports), a request that follows a redirect, sends credentials,
  cookies, or an environment proxy, or escapes its in-flight, timeout, or
  response-head bounds; inbound, the built-in listener (which binds
  127.0.0.1 unless told otherwise and speaks plain HTTP), including request
  framing and smuggling (`Content-Length` and `Transfer-Encoding`),
  bypasses of its connection, head, header, body, and timeout bounds, and a
  message that reaches a session other than the running one its target
  names. The listener has no authentication: any client that can reach it
  can queue events on every running session it serves, so expose it only
  behind a trusted proxy or tunnel. A `--record` log of a networked session
  holds every inbound message's payload, raw text, and W3C carrier as
  received, and a replay that touches the network (connects, listens, or
  answers an outbound request other than from the log) is in scope;
- the browser host (`hosts/scxml-host-browser`): bypasses of the page's
  `resources` and `http` allowlists (including URL forms that reach a
  different URL than the one `allow` judged), inbound `receiveHttp`
  decoding, the async worker session's `SharedArrayBuffer` channel, and
  bypasses of the build-time fuel metering of browser core modules;
- the offline conformance preparation sandbox (`conformance prepare
  --offline`), including transformer argument or stylesheet screening bypasses;
- repository namespace and service acquisition (`https://github.com/<owner>/<repo>/namespaces/<name>[@<tag>]`
  and `.../services/<name>[@<tag>]` resolved by the CLI), and the documents
  `snapshot run` and `run` load by URI (`<invoke src>`, `<script src>`,
  `<data src>`, `srcexpr`): a fetch over plain HTTP, with credentials, past
  its size, redirect, or time bound, or under `--offline`: a manifest, schema, or component accepted
  without matching its pinned commit and digests or its release's
  `SHA256SUMS`, a cache entry accepted after tampering, a fetch under
  `--offline` or for a pinned, cached URI, a redirect followed to a
  non-GitHub, non-configured origin, a credential sent anywhere but the
  configured API origin, or a bypass of the size, redirect, or timeout
  bounds. The trust boundary is what a document imports: binding or loading
  an element consents to the origins its manifest declares, at the release
  tag its URI names (or, for a tagless URI, the latest release, listed
  with its tag and commit), pinned to the commit of its resolution;
- isolation between extension elements and of the run's values: an extension
  component that obtains a value (rather than a reference), a reference
  bound to another element or valid after its `execute` call, a value
  substituted into a request to an origin neither the element's package
  declares nor a value supplied entirely, a value that reaches CLI output
  unmasked, or a document or component reaching a variable no `--env`,
  `--env-file`, or embedder value supplied.

Known, documented limitations are not vulnerabilities by themselves, but a
report showing practical impact beyond what is documented is welcome. One known
limitation is that native (non-Wasm) Boa ECMAScript evaluation has no
per-expression heap limit; see
[docs/limits.md](docs/limits.md#the-boa-bounding-gap).

Out of scope: vulnerabilities in third-party dependencies with no demonstrated
impact on this project (report those upstream), and the vendored W3C test
corpus under `tests/conformance/w3c-irp`, which is test data.
