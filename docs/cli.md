# Command-line interface

`scx` (crate [`crates/scxml-cli`](../crates/scxml-cli)) validates,
compiles, and runs SCXML documents. Run it from the workspace with
`cargo run -p scxml-cli -- <command>`. Commands that execute a document run it
only through the shipped Wasm components under the Wasmtime host (the
default `wasmtime` feature). `scx` embeds the authenticated release session
components: building it reads the release `scripts/verify-wasm-capabilities.sh`
writes (`target/wasm-artifacts/release`, or `SCXML_WASM_ARTIFACTS_DIR`),
checks every component's SHA-256 against its `manifest.tsv`, and fails with a
message naming what is missing rather than building without them; each run
authenticates them again against that manifest's SHA-256.
`--components DIR --components-manifest-sha256 HEX` overrides them with
another release directory and its trusted manifest digest (the
[README quickstart](../README.md#quickstart) sets `$RELEASE` and
`$MANIFEST_SHA256` for the commands that still name a release, such as
namespace suites).

| Command | Does | Needs `wasmtime` |
| --- | --- | --- |
| `validate DOC` | XML, the W3C SCXML 1.0 XSD, and semantic rules; rustc-style, JSON, or SARIF output | no |
| `compile DOC` | validate and lower to Machine IR (`lowered-ir`), or package a component (`component-wasm`) | no |
| `--explain CODE` | the long-form explanation of a diagnostic or `SNAP-` code | no |
| `snapshot run DOC` | run scripted events and write session snapshots ([session-snapshot.md](session-snapshot.md)) | yes |
| `snapshot show`, `check`, `diff` | render, check, and diff snapshot documents | no |
| `run DOC` | one live session over newline-delimited JSON ([live-session.md](live-session.md)) | yes |
| `conformance prepare` | prepare the official W3C IRP corpus ([conformance.md](conformance.md)) | no |
| `conformance run` | run a prepared corpus through the components and judge it | yes |

Fetching a repository namespace over the network also needs a CLI built with
`--features wasmtime` ([below](#loading-an-element-from-a-repository)).

## Validator diagnostics and reporters

Diagnostics carry deterministic severity, stable code, category, primary and
secondary byte labels, notes, actionable help, specification references, and
clippy-style applicability plus replacement edits where the repair is honest.
The versioned artifact stores authoritative error/warning/note/total counts and
rejects unknown versions/kinds, unknown fields, inconsistent counts, invalid
UTF-8 byte spans, and overlapping edits.

The semantic inventory covers (the generated table in
[validator-rules.md](validator-rules.md) is the authoritative list):

- `SCXML001`: root expanded QName;
- `SCXML002`/`003`: duplicate and NCName identifiers with related spans and fixes;
- `SCXML004`/`005`/`006`: transition shape (at least one of `event`, `cond`, or `target`, SCXML 1.0 §3.5.2) and target/initial references with typo suggestions;
- `SCXML007`/`008`: `param` and `cancel` required/XOR constraints;
- `SCXML009`/`010`: one constrained initial transition;
- `SCXML011`/`012`: state/root initial conflicts and atomic-state misuse;
- `SCXML013`: malformed or comma-separated event descriptor lists;
- `SCXML014`/`015`: `send` attribute pairs and `namelist`/child exclusions;
- `SCXML016`: `invoke` source/type and data-source exclusions;
- `SCXML017`: initial attributes targeting outside their containing subtree;
- `SCXML018`: `donedata` content/parameter exclusion;
- `SCXML019`-`SCXML022`: history/initial cardinality, unconditional target constraints, descendant targets, and shallow-history direct-child targets;
- `LINT001`-`LINT003`: implicit compound initialization, no-exit states, and possible indefinite conditional-eventless wait warnings (not normal event-triggered reactive waits);
- `LINT004`: reliably detectable unconditional eventless transition cycles (warning: SCXML 1.0 Appendix D allows a macrostep that does not terminate);
- `LIMIT001`/`LIMIT002`: bounds on nested inline documents and on machine size for graph analysis;
- `SCXML030`-`SCXML032`: incompatible multi-target sets, illegal `<final>` children, and misplaced `donedata`;
- `LINT005`-`LINT008`: unreachable states, single-region `parallel`, shadowed transitions, and eventless transitions that loop through default entry (warnings like `LINT004`);
- `SCXML037`-`SCXML039`, `SCXML079`, `LIMIT003`/`LIMIT004`, `PKG005`, `LINT009`, and `LINT010`: data-model rules (null data-model misuse: every element SCXML 1.0 §B.1.7 does not support is an error, and every expression there the `SCXML079` warning; expression and script syntax and assignable locations; checker bounds (`LIMIT`); a data model that is neither built in nor available from a package (`PKG005`); empty expressions and unchecked data models (`LINT`); syntax, location, empty-expression, and undeclared-XPath-variable findings are warnings, because SCXML 1.0 defines their run-time outcome, `error.execution`, §5.9);
- `SCXML050`/`SCXML051`: `<script>` with both `src` and content, and unfetchable `src`;
- `SCXML052`/`SCXML053`: a `<send>` to the SCXML event processor with no event, and a `<data>` or `<content>` with more than one value source;
- `SCXML055`/`SCXML056`: `<raise>`/`<send>` inside `<finalize>`, and XPath references to undeclared variables (warning);
- `LINT011`/`LINT012`: XPath relative paths that select no data, and `In()` calls naming no state or an unquoted state ID (warnings);
- `LINT013`/`LINT014`: XPath positional predicates no node satisfies, such as `[0]` (warning), and `+` on an XPath string, which is numeric addition (warning);
- `LINT015` (warning): an ECMAScript `<data id>` that is not an identifier (`retry-count`, `class`): the data model declares it (SCXML 1.0 §B.2.1), but no expression can name it, so a rename of the declaration and every reference is offered;
- `LINT016`/`LINT017`: a Unicode bidirectional control character that changes how the source is displayed (Trojan Source; warning), and an ECMAScript condition that is an arrow function and so always true (warning);
- `SCXML064`-`SCXML067`: a `<send>`, `<invoke>`, or `<state>` that repeats a child it allows once, an `<assign>` whose value does not fit its `type`, an empty required expression attribute, and an `<scxml>` with no child states (a `<parallel>` with none is conformant and only the `LINT021` warning);
- `LINT022` (warning): an attribute or element in another namespace that no package or registered schema binds, which has no effect (`xml:*` and `xsi:*` attributes are accepted silently, and `xml:base` governs the `src` URIs in its scope);
- `DATAMODEL001`/`DATAMODEL002`: the `datamodel` vocabulary used anywhere but `<scxml>`'s `datamodel:src`, and a `datamodel` token that is not the name of the package `datamodel:src` names;
- `LIMIT005`: a document that exceeds a Machine IR bound;
- `SCXML070`: a `delay` or `delayexpr` on a `<send>` to `#_internal`, which SCXML 1.0 §6.2 forbids;
- `SCXML071`: an `<invoke>` with both a `namelist` and `<param>` children, which SCXML 1.0 §6.4 forbids;
- `LINT018`: a state listed twice in one target list, which SCXML 1.0 §3.11 allows (a set of states) and the compiler enters once (warning);
- `SCXML073`: a `<data id>` that begins with `_` (SCXML 1.0 §5.10) under every data model but null;
- `SCXML074`, `LINT019`, `LINT020`, and `SCXML077`: an ECMAScript `<log expr>` that certainly has a side effect (SCXML 1.0 §4.7.3), found by the data model's parse-only analysis across the document's static scripts; one whose side effects the analysis cannot decide (warning); a condition that certainly has one (warning: §5.9.1's SHOULD NOT, since official IRP test 444 contradicts Appendix B's MUST, docs/spec-errata.md §5); and (warning) an assignment, update, or `delete` of a system variable itself, which fails at run time with `error.execution` (SCXML 1.0 §5.10);
- `LINT021` (warning): a `<parallel>` with no child state, which SCXML 1.0 §3.4.2 allows but through which no transition is ever enabled and which raises no `done.state`;
- `SCXML080` (warning): a multi-target set that names a `<history>` and a state below that history's parent, which yields a legal configuration (SCXML 1.0 §3.11) only when the history's configuration contains the other target;
- `LIMIT006`: a state ID longer than 1,024 bytes, which no configuration, snapshot, or `In()` can name on any build, or, for a state that can complete, longer than 1,013 bytes, so its `done.state.<ID>` event exceeds the 1,024-byte event-name bound (docs/spec-errata.md L.6);
- `LIMIT007`: a literal `<raise event>`, `<send event>`, `<send id>`, or `<invoke id>` longer than 1,024 bytes, or an `<invoke id>` longer than 1,012 bytes, whose `done.invoke.<ID>` event exceeds the event-name bound (docs/spec-errata.md L.6);
- `XSD-*`: schema setup and spec-aligned instance constraints;
- `XML001`-`XML006`: one code per XML failure kind (malformed, resource limit,
  undefined entity, invalid character reference, invalid caller limits, and an
  entity the parser does not read).

Rendering follows rustc: primary `^^^` and secondary `---` labels on the most
specific bytes, `= note:`/`= help:`/`= spec:` footers, and inline `help:`
suggestion snippets. `scx --explain CODE` prints the long-form
explanation of any code; the code table in
[validator-rules.md](validator-rules.md) is generated from the same
source.

### Agent interface

AI repair loops should consume the canonical artifact, not the text:

```console
scx validate machine.scx --format json   # canonical scxml.report bytes on stdout
scx validate machine.scx --format sarif  # SARIF 2.1.0 with fixes on stdout
scx --explain SCXML005                     # long-form explanation
```

The exit status is `0` when the report has no errors, `1` when it has errors
(diagnostics were produced), and `2` for usage, I/O, or internal failures.
Each diagnostic carries `code`, `severity`, `category`, exact UTF-8 byte spans,
`notes`, `help`, `spec_references`, and `fixes[]` with `applicability`
(`machine-applicable` edits may be applied unattended; `maybe-incorrect` and
`has-placeholders` need judgment). Apply fixes, re-validate, and repeat until
the report has no errors.

Executable-content extensions remain preserved and structurally governed by
their exact namespace XSD rather than rejected by core semantic rules. Remaining
semantic gaps and the AgentML comparison are listed in
[validator-rules.md](validator-rules.md#exact-remaining-gaps).

## Examples

### Validate and compile

```console
cargo run -p scxml-cli -- validate machine.scx \
  --schema http://www.w3.org/2005/07/scxml=scxml.xsd \
  --schema urn:vendor:extension=extension.xsd \
  --policy strict \
  --canonical machine.scxml-report.json

cargo run -p scxml-cli -- compile machine.scx \
  --output machine.scxml-ir.json \
  --schema http://www.w3.org/2005/07/scxml=scxml.xsd
```

`validate`, `compile`, `snapshot run`, and `run` read every JSON Schema file
an `event:schema` attribute references
([namespaces/event](../namespaces/event/README.md)) with no flag, each
resolved against the document's base URI as a `<script src>` is (its `file:`
URL; `..` segments resolve, so a file may be outside the document's
directory); a file they cannot read, or a reference that is not a relative
path, is `EVENT004`.
`snapshot run` and `run` load the files of every `<invoke src>` document
the same way, resolved against that child's own base URI (read from its
directory for a `file:` child, fetched for an `https:` one), and the host
compiles the child with them: a literal child's before the session, a
`srcexpr` child's when its `<invoke>` runs. `validate` and `compile`
also read each `file:` document a literal `<invoke src>` of the document
names (fetching nothing), with its own schema files, and `snapshot run` and
`run` pass the documents they loaded, so a `<send>` between the two whose
literal data the receiving transition's schema rejects is `EVENT011`.
All four also read each `file:` script a `<script src>` of the document
names (fetching nothing for validation), so the side-effect analysis of
conditions and `<log expr>` reads the functions it declares; a remote
script stays unread, and a call it may define or replace is `LINT019`.

### Package a component for an ahead-of-time host

```console
# Package for a non-browser AOT host: the template is the pinned virtualized
# generic machine (for example from scripts/verify-wasm-capabilities.sh).
cargo run -p scxml-cli -- compile machine.scx \
  --artifact component-wasm \
  --machine-template target/wasm-artifacts/release/scxml_machine.virtualized.wasm \
  --machine-template-sha256 <pinned-sha256> \
  --output machine.component.wasm \
  --canonical machine.component.report.json
```

### Pin a Wasm script component

```console
# Pin a Wasm script component for <script src="example.wasm"/> (compile-time
# resolution); add --script-resolution runtime to leave src to the host loader.
cargo run -p scxml-cli -- compile script-machine.scx \
  --script example.wasm=build/example.wasm \
  --script-sha256 example.wasm=<pinned-sha256> \
  --output script-machine.scxml-ir.json
```

### Prepare and run the W3C IRP

```console
cargo run -p scxml-cli -- conformance prepare \
  --manifest tests/conformance/w3c-irp/upstream/manifest.xml \
  --base-uri https://www.w3.org/Voice/2013/scxml-irp \
  --source-dir tests/conformance/w3c-irp/upstream/sources \
  --templates tests/conformance/w3c-irp/templates \
  --datamodel ecmascript \
  --cache tests/conformance/w3c-irp/upstream \
  --output tests/conformance/w3c-irp/prepared/ecmascript \
  --revision 2015-03-10 \
  --saxon saxon \
  --offline \
  --errata tests/conformance/w3c-irp/errata/ecmascript.json
# XPath: --datamodel xpath --output tests/conformance/w3c-irp/prepared/xpath \
#        --errata tests/conformance/w3c-irp/errata/xpath.json

cargo run --release -p scxml-cli --features wasmtime -- conformance run \
  --corpus tests/conformance/w3c-irp/prepared/ecmascript \
  --allow-not-passing \
  --json /tmp/scxml-conformance.json \
  --report-copy /tmp/scxml-conformance.copy.json
```

### Session snapshots

```console
# Read-only session snapshots (docs/session-snapshot.md): run the quickstart's
# hello.scx through its release components ($RELEASE) and write one
# canonical scxml.session-snapshot/v2 document per checkpoint to
# out/NNNN.snapshot.json, then render, check, and diff them.
cargo run --release -p scxml-cli --features wasmtime -- snapshot run target/hello.scx \
  --event finish --every --output-dir out
cargo run -p scxml-cli -- snapshot show out/0002.snapshot.json
cargo run -p scxml-cli -- snapshot check out/0002.snapshot.json
cargo run -p scxml-cli -- snapshot diff out/0000.snapshot.json out/0002.snapshot.json
```

### Executable content extensions

```console
# Extension namespaces: bind the example package (its schema validates
# example:*, and compile requires it), then run its extension component,
# authenticated by its SHA-256.
EXAMPLE_URI=https://stateforward.org/scxml/example
EXAMPLE_SHA256=$(awk -F'\t' '$2 == "scxml_extension_example" { print $5 }' "$RELEASE/manifest.tsv")
cargo run -p scxml-cli -- validate tests/fixtures/namespaces/example.scx \
  --extension-package components/scxml-extension-example/package/example.extension.json
cargo run --release -p scxml-cli --features wasmtime -- snapshot run tests/fixtures/namespaces/example.scx \
  --extension-package components/scxml-extension-example/package/example.extension.json \
  --extension-component "$EXAMPLE_URI=$RELEASE/scxml_extension_example.virtualized.wasm" \
  --extension-component-sha256 "$EXAMPLE_URI=$EXAMPLE_SHA256"
```

### A live session with record and replay

```console
# A live session (docs/live-session.md): newline-delimited JSON events in,
# one JSON record per line out (opened, log, call, snapshot, done, error,
# closed). Delayed sends fire at their wall-clock deadline; the run
# exits 0 when the root reaches a top-level final state. --record writes a
# replay log that --replay reproduces byte for byte on the logical clock.
printf '%s\n' '{"name":"finish"}' | \
  cargo run --release -p scxml-cli --features wasmtime -- run target/hello.scx \
    --snapshot-on done --record target/hello.live.ndjson
cargo run --release -p scxml-cli --features wasmtime -- run target/hello.scx \
  --replay target/hello.live.ndjson
```

### BasicHTTP between two processes

```console
# BasicHTTP between two processes (SCXML C.2). A listens on 127.0.0.1:8401
# and may post only to 127.0.0.1:8402; B listens on 8402, may post only to
# 8401, and pings A with its own location as the reply address. Each exits
# 0 once its root finishes.
cat > target/pong.scx <<'EOF'
<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="idle">
  <state id="idle">
    <transition event="ping" target="done">
      <log label="ping from" expr="_event.data.from"/>
      <send event="pong" type="basichttp" targetexpr="_event.data.reply">
        <param name="n" expr="7"/>
      </send>
    </transition>
  </state>
  <final id="done"/>
</scxml>
EOF
cat > target/ping.scx <<'EOF'
<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="waiting">
  <state id="waiting">
    <onentry>
      <send event="ping" type="basichttp" target="http://127.0.0.1:8401/sessions/a/basichttp">
        <param name="from" expr="'b'"/>
        <param name="reply" expr="_ioprocessors['basichttp'].location"/>
      </send>
    </onentry>
    <transition event="pong" target="done"><log label="pong" expr="_event.data.n"/></transition>
    <transition event="error.communication" target="lost"/>
  </state>
  <final id="done"/>
  <final id="lost"/>
</scxml>
EOF
SCXML_RUN="cargo run --release -p scxml-cli --features wasmtime -- run"
$SCXML_RUN target/pong.scx \
  --session-id a --http-listen 8401 --http-allow http://127.0.0.1:8402 < /dev/null &
$SCXML_RUN target/ping.scx \
  --session-id b --http-listen 8402 --http-allow http://127.0.0.1:8401 < /dev/null
# A logs {"label":"ping from","text":"b"}, B logs {"label":"pong","text":"7"};
# both report the delivering call as {"type":"call","cause":"http",...}.
# Any HTTP client can drive a listening session the same way:
#   curl --data-urlencode _scxmleventname=ping --data-urlencode from=curl \
#     http://127.0.0.1:8401/sessions/a/basichttp
```

## Flags and exit status

`compile` writes the artifact to `--output` and its recipe to
`<output>.recipe.json`, and with `--canonical` also writes the canonical
compiler report. The default `--artifact lowered-ir` writes portable Machine IR
data; `--artifact component-wasm` requires `--machine-template` and
`--machine-template-sha256` and writes a validated component;
`--artifact core-wasm` is rejected because it is never produced. `conformance run` exits nonzero by default unless
all applicable selected mandatory official cases are Passed.
`snapshot check` exits 1 when the document has `SNAP-<code>` issues
(`--explain SNAP-<code>` explains each), and `snapshot diff` exits 1 when
the documents differ; the document, diff, and error codes are specified in
[session-snapshot.md](session-snapshot.md).
`--extension-package MANIFEST.json` (repeatable) binds one extension namespace
for `validate`, `compile`, `snapshot run`, and `run`. `snapshot run` and `run`
also execute each bound namespace's extension component: `extension-artifact`
relative to the manifest, or `--extension-component URI=PATH`, authenticated by
the required `--extension-component-sha256 URI=HEX` (the manifest is not a trust
anchor for the extension component). Each extension component call runs
under the host's `DEFAULT_EXTENSION_CALL_FUEL` instruction cap (one billion,
enough for an element that compiles the whole W3C SCXML XSD set in one call;
its documentation records the measurement). A namespace whose URI is a repository
element needs no flag ("Loading an element from a repository" below).
An extension element is executable content and does no I/O: its manifest
declares no outbound origins and no `env` slots, and it receives no value.
The document scopes every variable: an invoked service's `<invoke>` names
the variable for one of its slots (`env:<slot>="NAME"`), and the variable
goes only to the services (the `<invoke>`'s literal `type`) whose
`<invoke>`s name it. An `env:<slot>expr` slot names its variable only when
its `<invoke>` runs, so its service may receive any variable the run
holds. `--env-file PATH` (repeatable,
Docker's `--env-file` format: one `NAME=VALUE` or bare `NAME` per line, the
value taken literally, `#` comments) and `--env NAME` or `--env NAME=VALUE`
(repeatable, once per `NAME`) supply values. A variable holds its flag's
`VALUE`, else the last `--env-file` entry for it, else, only when `--env NAME`
names it, the calling shell's value; a variable no slot can name is dropped.
Nothing is held by default. The service component receives an opaque
reference for its whole invocation, never the value, and a slot whose
variable holds no value is absent (see
[invoked-services.md](invoked-services.md#values-are-references-env-slots)), and every value is masked (`[masked]`) in the text the CLI
writes (notes, diagnostics, live-session records; in a record only inside
its JSON strings, never in a literal or a number, so a value such as `true`
leaves every record one JSON line); canonical snapshot
documents are written exactly (their `sha256` covers their bytes) and get
the protections of the canonical artifacts they are.
Every command that reads a document (`validate` in its human format, which
keeps JSON and SARIF stderr clean) lists, on standard error, each bound
element's namespace and version (release tag and pinned commit, or package
version and manifest). `snapshot run` and `run` then list each variable the
document names and the `<invoke>`s it goes to (`env `NAME``, `= to: <invoke
type="URI"> env:SLOT`, noting a variable the run holds no value for), and
each `env:<slot>expr` slot; names and URIs only, never a value.
A literal `<invoke type>` of exactly
`https://github.com/OWNER/REPO/services/NAME[@TAG]` that no `--service-package`
binds is a repository service: it is resolved, verified, cached, and pinned
exactly like a repository namespace (the same `--offline`, `--cache-dir`, and
`--github-*` flags), its component authenticated by the release's
`SHA256SUMS` when `snapshot run` or `run` binds it
([invoked-services.md](invoked-services.md#loading-a-service-from-a-repository)).
`--service-package MANIFEST.json` (repeatable) binds one invoked service
(SCXML 6.4: an `<invoke type>` that is neither absent nor the SCXML type)
for `validate`, `compile`, `snapshot run`, and `run`: a
`stateforward.scxml.service-package/v1` manifest with exactly the fields
`format`, `service-uri`, `package-version`, `service-artifact`,
`schema-artifact` and `schema-sha256` (both a string or both `null`; a named
schema is read relative to the manifest and verified against its digest),
`wit-world` (`stateforward:scxml/invoked-service`), `wit-version`,
`http-origins`, and `env-slots`, and optionally `validator-artifact` and
`validator-sha256` (both or neither: a package validator component, read
relative to the manifest and verified against its digest, which
`validate`, `compile`, `snapshot run`, and `run` call on the document's
`<invoke>`s of the service; [validator-rules.md](validator-rules.md#package-validators));
unknown fields and values are rejected
(a manifest declares no WASI and no events). `validate` warns on a literal
`<invoke type>` no package binds (`PKG004`), and on a literal type that is
a platform-specific value rather than a URI (`PKG003`); `compile` rejects
neither, and the `<invoke>` raises `error.execution` when it runs (SCXML 6.4:
invocation is a runtime action). `snapshot run` and `run` execute each bound
service's component (`service-artifact` relative to the manifest, or
`--service-component URI=PATH`), authenticated by the required
`--service-component-sha256 URI=HEX`, and list each bound service's URI,
version, declared origins, slots, the WASI interfaces its component imports
(`= WASI imports:`), and the WASI resources the run grants it
(`= WASI resources:`) before running.
A data model other than the built-in `ecmascript`, `null`, and `xpath` is a
package ([namespaces/datamodel](../namespaces/datamodel/README.md)): `<scxml datamodel="NAME"
datamodel:src="URI">` (the `datamodel` vocabulary,
`https://github.com/stateforward/scxml.rs/namespaces/datamodel`) names its
package URI, and `NAME` must be the name the package declares
(`DATAMODEL002`). `--data-model-package MANIFEST.json` (repeatable;
`validate`, `compile`, `snapshot run`, and `run`) binds one: a
`stateforward.scxml.data-model-package/v1` manifest with exactly the fields
`format`, `data-model-uri`, `datamodel`, `package-version`,
`data-model-artifact`, `wit-world` (`stateforward:scxml/data-model`), and
`wit-version`. A `datamodel:src` of exactly
`https://github.com/OWNER/REPO/datamodels/NAME[@TAG]` that no
`--data-model-package` binds is a repository data model, resolved, verified,
cached, and pinned exactly like a repository service; one that does not
resolve, or any other URI no package binds, is `PKG005`. `snapshot run` and
`run` execute each bound data model's component (`data-model-artifact`
relative to the manifest, or `--data-model-component URI=PATH`,
authenticated by the required `--data-model-component-sha256 URI=HEX`; a
repository data model's from its release, authenticated by its
`SHA256SUMS`) as the data model of every session whose `datamodel` names it,
and list each bound data model's name, URI, and version before running.
A service's WASI interfaces are its component's own `wasi:*` imports; only
runtime resources need a grant, with wasmtime CLI's option names scoped to
the service bound to exactly `URI` (up to the first `=`; `snapshot run` and
`run`, repeatable): `--dir URI=HOST::GUEST` preopens `HOST` read-write at the
absolute guest path `GUEST` for a component importing `wasi:filesystem`,
`--wasi URI=inherit-network` lets `wasi:sockets/tcp` connect to any address,
and `--wasi URI=allow-ip-name-lookup` allows `wasi:sockets/ip-name-lookup`.
An import without its grant, a grant without its import, and a `wasi:*`
import the host never links (`wasi:cli`, `wasi:http`) fail closed when the
service is bound, before anything runs
([invoked-services.md](invoked-services.md#wasi-imports-and-resources)).
A run holding a live service cannot be snapshotted (the snapshot is
refused). A `--record` log binds each service's URI and component SHA-256,
and `--replay` refuses a run whose services differ
(`[replay-identity-mismatch]`). A service's `outbound-http.fetch` suspends
only the calling task, never the session
([invoked-services.md](invoked-services.md#outbound-http)): `run` performs
each request on a transport thread of its own (no time limit of its own,
at most 16 in flight per invocation), the session keeps
taking input meanwhile, and the response is delivered by its own call
(cause `http-response`); on `--clock logical` no delayed send fires while a
request is in flight. `--record` records each response and `--replay`
delivers it from the log without the network, for services that import
WASI interfaces too. Leaving the invoking state cancels the invocation
(SCXML 6.4) and closes its request's connection at once: that, and the
document's own `<send delay>` timers, bound a request; an unreachable host
fails when the operating system gives up connecting. A service's
`outbound-websocket` connections run the same way, through the WebSocket
client (and `--websocket-proxy`) that processor connections use; each
answer is a `websocket` record without `processor`, and a run with an open
connection does not end at the end of its input
([invoked-services.md](invoked-services.md#outbound-websocket)).
## `run`

`run` keeps one live session of a document open through the same components
and flags ([live-session.md](live-session.md)). It processes each
standard-input event to a stable configuration when it arrives. With `--clock wall`, the
default, delayed sends fire at their real deadline, and `--clock logical`
fires them right after each event. At the end of input `--on-eof wait` keeps
firing delayed sends until nothing is scheduled, and `--on-eof close` closes
at once. `SIGINT` and `SIGTERM` close the session gracefully (exit 130 and
143). The exit status is 0 when the root finishes, 1 when the session fails,
2 for usage, compilation, and I/O failures, and 3 when the session closes
while running. `--processor-package`, `--processor-component`,
`--processor-component-sha256`, and `--websocket-proxy` bind Event I/O
Processor components ([event-io-processors.md](event-io-processors.md)); a
session with an open processor connection does not close at the end of
input.

## Documents loaded by URI

`snapshot run` and `run` load `<invoke src>`, `<script src>`, and
`<data src>` by SCXML's defaults: an `https:` URL, a `file:` URL, or a
relative URI resolved against the base URI of the document that names it
(the run's document has its `file:` URL; an `<invoke src>` document the URL
it was loaded from). Every literal URI of the document, and of every
`<invoke src>` document it loads (at most 64 documents), is fetched once
before the first macrostep and listed before running:

```text
document `file:///work/documents/child.scx`
  = for: <invoke src="child.scx">
  = loaded: 312 bytes, sha256 …
```

A `<script src>` of the document that cannot be fetched rejects it (SCXML
1.0 §5.8); any other failure raises `error.execution` when its element runs.
An `<invoke srcexpr>` URI is fetched each time its `<invoke>` runs, through
the same fetcher, as the document is then (a document the run writes loads
as written), with the schema files and scripts it names, resolved against
its own URL; a failure raises `error.execution`. Remote documents come over
HTTPS only (16 MiB, at most 5 redirects, each to `https:`, the repository
resolver's timeouts, no retry, no credentials, cookies, or proxies), and
`--offline` forbids them. Every literal URI is fetched once per run and
shared by every session of the tree; each session is served the scripts its
own document names, and the repository namespaces and services of
every literal `<invoke src>` document are bound with the root's.

## BasicHTTP over the network

`run` speaks the BasicHTTP Event I/O Processor (SCXML C.2) over the network
only when asked; the shipped `scxml-ioprocessor-basichttp` component still
encodes every request, decodes every message, and chooses every status, and
the host only moves bytes. By default every POST outside the session's own
locations raises `error.communication` at once, as in a batch run.
`--http-allow ORIGIN_OR_PREFIX` (repeatable) admits an origin
(`https://api.example`) or a URL prefix (`https://api.example/hooks`, which
never admits `/hooks-admin`); every target is canonicalized with the WHATWG
URL parser before the check, so `..`, `%2e%2e`, and backslash segments cannot
leave an admitted prefix, and userinfo is refused. The request runs off the
session thread with no cookies, credentials, environment proxy, or
redirects, a 10 s connect and 30 s total timeout, and at most 64 in flight;
the send itself succeeds at once, and a transport failure or a non-2xx
answer arrives later as `error.communication` (`_event.type` `platform`,
SCXML 5.10.1) on the sender's internal event queue (SCXML 3.12.2, 6.2.4),
processed at once by its own macrostep (a step with cause
`communication-error`). `--http-listen PORT|IP:PORT`
(`PORT` binds 127.0.0.1) serves every session's location, the root's and
every invoked session's at any depth, advertised in `_ioprocessors` as
`<base>/sessions/<_sessionid>/basichttp`; the base is `http://ADDR`, or
`--http-public-url URL` behind a proxy or tunnel (required when binding
`0.0.0.0` or `::`). A POST to a running session is queued on its external
queue and processed at once (a call with cause `http`); the client gets the
component's status: 200 queued, 400 malformed, 405 not POST, 413 too large,
404 no running session there, 503 queue full. A listening session stays open
after the end of input. `--record` covers the network: inbound messages,
late failures, and every outbound accept-or-refuse decision are recorded
inputs, and `--replay` reproduces the session byte for byte without the
network (so the http flags cannot be given with `--replay`).

## Loading an element from a repository

A document can name an element by its package's path in a GitHub repository at
a release tag, the way agentml-go's namespaces were Go import paths, and the
CLI loads it: no flag binds it.

```xml
<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:weather="https://github.com/example-org/scxml-weather/namespaces/weather@v1.0.0"
       version="1.0" datamodel="ecmascript" initial="ask">
  …
  <weather:summarize location="forecast"/>
```

The URI is exactly `https://github.com/<owner>/<repo>/namespaces/<name>`, the
package's identity, which names the repository's latest release; a document
pins a release by writing `@<tag>` after it. The tag is split off when the
document is parsed and read only by the resolver: everything else sees the
tagless URI (the `env`, `event`, and `scx` vocabularies `scx` provides are
matched first, never fetched, and take no tag). `validate` and `compile` fetch the
package manifest (`namespaces/<name>/package/<name>.extension.json`) and its schema
from the commit the tag (or the latest release's tag) names; before the
session starts, `snapshot run` and `run` bind the release asset the manifest
names, verified against that release's `SHA256SUMS`. The manifest names the
tagless URI. Every byte is verified, cached
content-addressed under `${XDG_CACHE_HOME:-$HOME/.cache}/scxml/extensions`
(`--cache-dir DIR` to change), and the URI is pinned to the commit and
digests of its resolution (a tagless URI with its release tag); a pinned
URI's later runs re-verify the cache and use no network, a tagless URI asks
for the latest release once per run and re-pins when it moved, and
`--offline` forbids any fetch. Each resolution is made once per run; the
root document's resolution of a package (or, for a package the root does not
use, the first `<invoke src>` document's) is shared by every tagless use and
every invoked session, and an `<invoke src>` document that pins another
release binds that release for its own sessions. The namespaces of every
literal `<invoke src>` document are resolved with the root's. A malformed
pin, or one document pinning a package to two releases, fails closed. Fetching is the
GitHub resolver component `scx` embeds (WASI 0.3 `wasi:http`, admitted only
to the bases' origins and GitHub's asset hosts; the `--github-*-base` flags
are its configuration), which needs a CLI built with `--features wasmtime`;
a build without it resolves only what the cache holds. The CLI verifies
everything the component returns. An element does no I/O, so importing it
consents to no network access; a repository service consents to the origins
its manifest declares (`http-origins`). Any failure fails closed: validation stops, or the
run fails before the first macrostep.
[executable-content-extensions.md](executable-content-extensions.md#loading-an-element-from-a-repository)
specifies the URI, layout, trust model, cache, and bounds.

```console
# The token in a gitignored .env file (Docker's format).
printf 'WEATHER_TOKEN=%s\n' "$WEATHER_TOKEN" > .env
# First run: resolves, verifies, caches, and pins the element.
cargo run --release -p scxml-cli --features wasmtime -- snapshot run weather.scx \
  --env-file .env
# Later runs need no network at all.
cargo run --release -p scxml-cli --features wasmtime -- snapshot run weather.scx \
  --env-file .env --offline
```

The first command prints, before it runs, the element's namespace, its
release tag and pinned commit, the origins its manifest declares, and the
slots the document binds (names only, never a value).

A model call is not an extension element: it is slow I/O, which belongs in
an invoked service ([invoked-services.md](invoked-services.md)), loaded from
a repository the same way.
