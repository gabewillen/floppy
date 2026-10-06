# Wasm script components (`<script src>`)

SCXML 1.0 §5.8 gives `<script>` either a `src` URI or inline content, never
both. The data model defines the scripting language, and if the script named by
`src` cannot be fetched or parsed at document load the document is rejected.
This project follows that model and adds one media type: a `src` that resolves
to `application/wasm` must be a Component Model component implementing
`stateforward:scxml/script@0.2.0`.

```xml
<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="run">
  <script>var input = 21; var doubled = 0;</script>
  <state id="run">
    <onentry>
      <script src="example.wasm"/>
      <log label="doubled" expr="String(doubled)"/>   <!-- doubled: 42 -->
    </onentry>
  </state>
</scxml>
```

There is no non-standard `type` attribute: the loader resolves the URI to bytes
plus a media type, and the media type selects the handler.

| Resolved media type | Handler |
| --- | --- |
| `application/wasm` | Wasm script component (`script` world) |
| the data model's own script language (ECMAScript: `application/ecmascript`, `application/javascript`, `text/ecmascript`, `text/javascript`) | the data model's own `execute` |
| anything else | rejected (`unsupported-media-type`) |

Media types compare byte-for-byte; parameters and other spellings are rejected,
not normalized.

## The script world

A script component reuses the existing data-model access interface; there is no
new bridge:

```wit
interface script-api {
  run: func(event: option<event>) -> result<_, component-error>;
}
world script {
  import telemetry;
  import data-model-host;   // evaluate(expression), assign(location, value)
  export script-api;
}
```

The host links `data-model-host` to the session's active data model, so every
read and assignment operates on that data model's state, with `_event` bound to
the event of the executable content that ran the script. The component keeps no state: the
host instantiates it fresh, in its own bounded store (memory limits, per-run
fuel), for every run. The session host also charges each run to the fuel
budget of the host call that reaches it, which that call shares with every
other component it reaches (`DEFAULT_FUEL`; see [limits.md](limits.md#fuel)).
A run that returns `invalid-input` or `execution`, or that traps or exhausts
its fuel, raises SCXML `error.execution` and aborts the rest
of its executable-content block, exactly like a failing inline script; any
other error code is a contract violation that fails the session.

`components/scxml-script-example` is the reference component: each run reads
`input` and assigns twice its value to `doubled`; a non-numeric input is an
`execution` failure.

## One loader contract, two resolution paths

Both paths authenticate through the same check (`scxml_script::verify_script`):
URI bounds, media-type dispatch, byte bound (4 MiB per component on every
build; 64 KiB per text script in a wasm build only), exact SHA-256 against any pin, component encoding (a core module
or other bytes are rejected), and the `script` world (exports exactly
`script-api`; imports only `types`, `telemetry`, and `data-model-host`, so no
WASI or other ambient capability).

### Compile time (pinned)

The compiler request carries script packages: URI, media type, expected
SHA-256, and bytes. With `script-resolution: compile` (the default) every `src`
needs exactly one package for its exact URI; the compiler authenticates it and
pins it into the Machine IR (`src.pinned`: media type, SHA-256, and for Wasm the
world). Data-model-language text is embedded in the IR and checked against its
pin on every IR decode; Wasm components stay outside the IR (IR remains data).
The package recipe lists each pinned component as `script-sha256:<hex>`.

A missing, unused, duplicated, or invalid package is a `COMP003` error on the
exact `src` attribute value, with the §5.8 rejection note:

```text
error[COMP003]: no script package supplies `src` URI `example.wasm`
  --> tests/fixtures/script-machine.scx:16:20
   |
16 |       <script src="example.wasm"/>
   |                    ^^^^^^^^^^^^ unresolved script source
   |
   = note: SCXML 1.0 §5.8: if the script specified by `src` cannot be downloaded or parsed at document load time, the document is rejected
   = help: supply a script package for this exact URI (bytes, media type, and SHA-256), or compile with `runtime` script resolution so the host's script loader resolves it at load
```

```sh
scx compile machine.scx --output machine.scxml-ir.json \
  --script example.wasm=build/example.wasm --script-sha256 example.wasm=<hex>
```

`--script URI=PATH` splits at the last `=`; the media type follows the file
extension (`.wasm`, `.js`, `.mjs`).

For non-browser ahead-of-time hosts, packaging (`--artifact component-wasm`)
also bundles the pinned components: the packager checks each supplied
component against its pin, encodes a data-only `script-bundle-provider`
component, and the packaged machine additionally exports `script-bundle`.
Hosts serve pinned references from that bundle; the packaged artifact's own
authenticated digest is the trust anchor. A bundle holds at most 64 entries,
strictly sorted by valid URI; the Wasmtime host rejects the whole packaged
machine (no session is created) when the bundle breaks those bounds, and
checks each entry's digest, media type, and size at `load-script`.

### Run time (dynamic or agent-authored SCXML)

With `script-resolution: runtime` references stay unpinned and the host's
`script-loader` capability resolves them at document load:

```wit
interface script-loader {
  record loaded-script { media-type: string, bytes: list<u8> }
  resolve: func(uri: string) -> result<loaded-script, component-error>;
}
```

The embedder supplies the resolver; the host authenticates what it returns and
additionally requires its SHA-256 to be in the embedder's digest allowlist. The
Wasmtime host exposes this as `ScriptResolver` plus `ScriptPolicy` (allowlist,
byte bound, per-run fuel) passed to `execute_machine_session_with_scripts`;
each run is bounded by its per-run fuel and also charged to the host call's
shared budget.

**Browsers use only this path.** A browser page ships the generic machine once
and feeds IR as data, so it never loads packaged components; the page's
`loadScript`/`runScript` providers implement the same contract.

## Document-load semantics

The machine owns the semantics; the host is a thin linker. At session
construction the machine calls `machine-capabilities.load-script` once per
distinct URI (document order) for every reference whose content is not
embedded in the IR. The first failure starts the session `failed` with that
cause: the document is rejected (§5.8).

| Failure | Code |
| --- | --- |
| no loader linked, or the loader answered `unknown-id` or `unavailable` (loader codes are translated, not forwarded) | `unavailable` |
| invalid URI, one URI with two different pins, resolved media type differs from the pin, digest mismatch against the pin, digest not allowlisted, unknown media type, non-component bytes, world mismatch, text in another language or not UTF-8 | `invalid-input` |
| over the host byte bound (at most 4 MiB per component; 64 KiB per text script in a wasm build only) | `payload-too-large` |
| loader answered `platform` | `platform` |
| `src` and inline content together | validator `SCXML050` (and invalid Machine IR) |
| empty or control-character `src` | validator `SCXML051` |

Effects then plan `run-script(reference, event)` where the `<script>` appears;
running a reference that was never loaded is a `protocol` violation.

## Proof

- `crates/scxml-script`: verification unit tests (accepted world, digest
  mismatch, media types, core module, WASI import, wrong export, bounds).
- `crates/scxml-compiler-core/tests/script_packages.rs`: pinning, runtime
  resolution, `COMP003` spans for every hostile package, `SCXML050`.
- `crates/scxml-packager/tests/package_contract.rs`: bundling, exports,
  missing/unexpected/swapped scripts.
- `hosts/scxml-host-wasmtime/tests/script_components.rs`: the scripted fixture
  under Wasmtime compiled with the pinned script (packaged with the bundle, and
  generic machine plus loader), resolved at run time, and resolved to ECMAScript
  text, all with the identical trace (`doubled: 42`, `doubled: 84`,
  `error: error.execution`, `final: 84`); and every hostile load above rejected
  with its typed code.
- `scripts/verify-wasm-capabilities.sh scxml_machine scxml_script_example`:
  builds, virtualizes, and import-allowlists the example component, and proves
  the scripted package deterministic, import-allowlisted, and exporting exactly
  `machine-api` and `script-bundle`.

The conformance executor supplies the Wasmtime session host's
`script-loader` capability from each case's integrity-checked prepared
dependency set (see [conformance.md](conformance.md)); IRP case 301 (an
unfetchable `src`) is rejected at load and passes under its
manual-equivalent oracle.
