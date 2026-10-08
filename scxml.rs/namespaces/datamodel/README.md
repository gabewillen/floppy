# `datamodel` attribute vocabulary

`https://github.com/stateforward/scxml.rs/namespaces/datamodel` is the host's
attribute vocabulary for naming the package of a data model. It is not an
extension package. The namespace has one attribute and nothing else: no
elements, no executable content, no extension component, no package
manifest, and no release asset. It is never fetched; the toolchain
recognises the URI byte for byte (`scxml_types::DATAMODEL_NAMESPACE_URI`).
This folder exists so the URI names a real path in the repository, like
every other `namespaces/<name>` URI.

| Path | What |
|---|---|
| `package/datamodel.xsd` | the schema, for readers and tools that resolve a namespace to a schema (it declares `src` as a URI; the validator does not load it) |

## The attribute

```xml
<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:datamodel="https://github.com/stateforward/scxml.rs/namespaces/datamodel"
       version="1.0" datamodel="example"
       datamodel:src="https://github.com/stateforward/scxml.rs/datamodels/example">
```

- The core `datamodel` attribute stays what SCXML 1.0 §3.2.1 defines: a
  token (the schema's `NMTOKEN`) naming the data model the machine's
  expressions are written in. The built-in data models, `ecmascript`,
  `null`, and `xpath`, ship with the toolchain and need no `datamodel:src`.
- `datamodel:src` on `<scxml>` names the package of any other data model: a
  repository data model, `https://github.com/<owner>/<repo>/datamodels/<name>`
  (its package lives in `datamodels/<name>/package/` of the repository),
  optionally followed by one `@<tag>` that pins a release (split off when the
  document is parsed, exactly like a repository namespace's), or the URI a
  `--data-model-package` binds.
- The token must be the name the package declares (its manifest's
  `datamodel`).
- An invoked document declares its own pair; a tagless use shares the run's
  resolution of the package.

The vocabulary defines no other attribute. In particular there is no
`datamodel:srcexpr`: an expression needs a data model to be evaluated in, so
it cannot select one. A parent that computes a child document uses
`<invoke srcexpr>` or `<content expr>`, and the child declares its own
`datamodel:src`.

## Packages

A data-model package is a `stateforward.scxml.data-model-package/v1`
manifest, `<name>.data-model.json`, with exactly these fields:

| Field | What |
|---|---|
| `format` | `stateforward.scxml.data-model-package/v1` |
| `data-model-uri` | the package's tagless URI, which `datamodel:src` names |
| `datamodel` | the data model's name, which the `datamodel` token must equal (an `NMTOKEN`, none of the built-in names) |
| `package-version` | the package's version |
| `data-model-artifact` | the component, relative to the manifest (a release asset name for a repository data model) |
| `wit-world` | `stateforward:scxml/data-model`, the world the built-in data models implement |
| `wit-version` | the WIT package version |

The component implements the whole `data-model-api`, so its `check-syntax`
also answers the validator component's `expression-syntax` import when an
embedder links it (`ComponentValidator`). `scx validate` reads only the
manifest, so a package data model's expressions get the `LINT010` note.

[`datamodels/example`](../../datamodels/example/package/example.data-model.json)
is the example: the null data model's component repackaged under the name
`example`.

## Resolution and execution

`validate`, `compile`, `snapshot run`, and `run` resolve a repository data
model through the CLI's bounded resolver exactly like a repository service
(the same cache, pinning, `--offline`, and fail-closed rules;
[docs/cli.md](../../docs/cli.md)): the manifest at the release's commit when
the document is read, and the component from the release, verified against
its `SHA256SUMS`, when `snapshot run` or `run` binds it. A data model that is
neither built in nor bound by a package (one whose release does not resolve
included) is `PKG005`, so the document never runs without its data model.
The Wasmtime session host binds each package's component by the data
model's name (`SessionHost::with_data_model_components`) and gives every
session whose Machine IR names it its own instance. The browser host takes
the data model a page passes (`createSession({ dataModel })`); it resolves
no package at run time (see its README).

## Rules

- `DATAMODEL001`: an element in the `datamodel` namespace, a `datamodel`
  attribute on an element other than `<scxml>`, or an attribute other than
  `datamodel:src`.
- `DATAMODEL002`: the `datamodel` token is not the name the package
  declares.
- `PKG005`: a data model that is neither built in nor available from a
  package.

`scx --explain DATAMODEL001`, `--explain DATAMODEL002`, and
`--explain PKG005` give each rule with an example.
