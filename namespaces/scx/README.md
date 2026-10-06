# `scx` namespace

`https://github.com/stateforward/scxml.rs/namespaces/scx` is the SCXML
authoring namespace of the `scx` toolchain: `<scx:validate>` runs the
scxml.rs validator (XML well-formedness, the bundled W3C SCXML 1.0 XSD, the
semantic rules, and the `event:schema` checks) on a document a machine holds
as text, and assigns the canonical `scxml.report/v3` validator report to a
data-model location, optionally with the machine-applicable fixes applied. A
machine that writes SCXML (a builder bot, for example) checks its drafts with
it.

It is an extension package (`stateforward.scxml.extension-package/v2`), but
it ships inside `scx`: the binary embeds the manifest, the schema, and the
extension component (`scx.virtualized.wasm`, authenticated by the SHA-256
the release manifest lists for it, checked when `scx` is built). A document
that binds the namespace uses it with no flag and no fetch, unless an
`--extension-package` binds the same URI. Because it is versioned with the
toolchain, its URI carries no release tag, like the
[`env`](../env/README.md) and [`event`](../event/README.md) vocabularies;
`scripts/check-namespace-versions.sh` and
`scripts/set-namespace-version.sh` skip it.

| Path | What |
|---|---|
| `src/lib.rs` | the element's behaviour and the component's WIT glue (`stateforward:scxml/executable-content-extension`) |
| `src/tests.rs` | unit tests of the element over a fake data model |
| `package/scx.extension.json` | the package manifest: no `http-origins`, no `env-slots` |
| `package/scx.xsd` | the schema (structure only) |

`crates/scxml-cli/tests/scx_namespace.rs` runs it end to end through
`scx run` and `scx validate`.

## The element

```xml
<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:scx="https://github.com/stateforward/scxml.rs/namespaces/scx"
       version="1.0" datamodel="ecmascript" initial="check">
  <datamodel><data id="report"/><data id="fixed"/></datamodel>
  <state id="check">
    <onentry>
      <scx:validate location="report" expr="draft_text" name="draft.scx"
                    applyfixes="machine-applicable" fixedlocation="fixed"/>
    </onentry>
    <transition cond="report.validator.counts.errors === 0" target="valid"/>
    <transition cond="report.validator.counts.errors &gt; 0" target="invalid"/>
  </state>
  <final id="valid"/>
  <final id="invalid"/>
</scxml>
```

`src/lib.rs` documents every attribute and child (`location`, `expr` or one
`scx:content`, `name`, `applyfixes` with `fixedlocation`, and
`scx:eventschema` resources), the failures (`error.execution` with a static
detail; the document, its schemas, and the report are never echoed), the
wasm-only bounds, and the telemetry. Executable content is synchronous: one
`execute` call runs the whole validator, the XSD set's compilation included,
within the host's `DEFAULT_EXTENSION_CALL_FUEL`.
