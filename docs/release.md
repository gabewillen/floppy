# Release and Versioning

floppy releases are GitHub releases of the repository the packages' URIs
name, `github.com/stateforward/floppy`. A release is what a bot's tagless
repository URIs resolve to without `--offline --cache-dir`: `scx`'s resolver
reads the latest release's tag (or the `@<tag>` a document pins), resolves it
to a commit, reads each package's manifest and schema from the repository at
that commit, and downloads each component from the release's assets,
verified against the release's `SHA256SUMS` (scxml.rs
[executable-content-extensions.md](../scxml.rs/docs/executable-content-extensions.md#loading-an-element-from-a-repository)
and [invoked-services.md](../scxml.rs/docs/invoked-services.md#loading-a-service-from-a-repository)).

Nothing is released yet. The repositories are private, and no tag, release,
or `SHA256SUMS` upload happens until the maintainer says they are public.
Everything below is prepared and checked locally.

- Every package's version is its manifest's `package-version`:
  `namespaces/<name>/package/<name>.extension.json` and
  `services/<name>/package/<name>.service.json`. All packages share the
  repository's release, so a release tag such as `v0.1.0` matches every
  package version.
- `scripts/check-package-versions.sh TAG` validates that every package binds
  its tagless URI (`https://github.com/stateforward/floppy/namespaces/<name>`,
  `.../services/<name>`; decision D21), that a namespace schema's
  `targetNamespace` is that URI, that every `schema-sha256` matches its
  schema, and that every `package-version` is TAG's version.
- `scripts/build-canonical-components.sh --artifacts-dir DIR` builds the
  canonical release components: `scripts/build-components.sh` inside the
  official `rust:1.98.0-bookworm` image pinned by digest and forced to
  `linux/amd64`, with `wasm-tools` 1.244.0, `wasi-virt` at its pinned
  revision, and wasi-sdk 25.0 checked by SHA-256. Release component bytes
  depend on the build host's target triple (decision D27), so the Linux
  x86_64 build is the canonical identity, as it is for scxml.rs. It then
  checks that the build reproduces the digests the checked-in manifests pin
  for components (the generate package's `validator-sha256`), and fails
  naming the canonical digest when it does not.
- `scripts/prepare-release.sh TAG --artifacts-dir DIR --release-dir OUT`
  runs the version check, copies the assets the manifests name
  (`extension-artifact`, `service-artifact`, `validator-artifact`) from DIR,
  refuses a package validator whose bytes are not the manifest's
  `validator-sha256`, writes `OUT/SHA256SUMS` in `sha256sum` format, and
  copies the licenses. It publishes nothing.
- `scripts/check-release-layout.sh TAG --release-dir OUT [--commit REV]
  [--scx PATH]` is the dry run: it lays out a GitHub mirror in a temporary
  directory (the API answers for the latest release and the tag, the
  packages' files at REV, OUT as the release's assets), serves it on
  loopback, and runs `scx validate` on every bot and ability document and
  `scx run` on every bot root against it with an empty cache, then checks
  that every repository URI the documents name was pinned to TAG and REV
  with a component `SHA256SUMS` lists.

## Why This Shape

- The packages are the release, so the release carries only what the
  resolver fetches from assets: the components and `SHA256SUMS`. Manifests
  and schemas come from the repository at the tag's commit, never from
  assets.
- The canonical build is a container, not the maintainer's machine: a
  macOS build of the same sources has other bytes, and the `validator-sha256`
  a manifest pins must be the bytes the release ships, or every run that
  resolves the generate package fails closed.
- The dry run exercises the real resolver against the real layout (through
  scxml.rs's `--github-*-base` mirror flags, which accept plain http only for
  loopback), so a missing asset, a wrong `SHA256SUMS` line, or a manifest
  naming the wrong artifact fails before anything is public.

## Release Flow

1. Update every package's `package-version` that will be released, and
   commit.
2. Run `sh scripts/build-canonical-components.sh --artifacts-dir "$PWD/canonical-release"`.
   If it reports a `validator-sha256` to record, record it in the manifest,
   commit, and build again.
3. Run `sh scripts/prepare-release.sh vX.Y.Z --artifacts-dir canonical-release --release-dir release`.
4. Build `scx` (README.md, "Running a bot") and run
   `sh scripts/check-release-layout.sh vX.Y.Z --release-dir release --scx "$(command -v scx)"`.
5. Once the repositories are public, and only then: push the tag
   `vX.Y.Z` on the commit of step 1 and create its GitHub release with
   every file in `release/` (`gh release create vX.Y.Z --verify-tag -- release/*`).

## Open Decisions Before External Publish

- Where the release lives: the packages' URIs name
  `github.com/stateforward/floppy`, so the release must be made there (the
  private repository is `gabewillen/floppy`).
- Whether to add a tag-triggered release job, as scxml.rs has, that runs
  steps 2 to 4 on a Linux x86_64 runner and publishes; today the flow is
  manual.
- floppy's packages need an `scx` that understands WIT `@0.2.0` and Machine
  IR `5`, so a floppy release follows a scxml.rs release of them.
