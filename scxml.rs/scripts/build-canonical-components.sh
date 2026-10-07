#!/bin/sh
# Builds the canonical release components (docs/conformance.md, "Canonical
# release identity") on any Docker host, so the exact IRP baseline identity
# bytes can be reproduced and checked without the `irp-components` CI job.
#
# Usage: sh scripts/build-canonical-components.sh --artifacts-dir DIR
#
# The canonical identity is the Linux x86_64 build: release component bytes
# depend on the build host's target triple (docs/conformance.md), so this
# script builds inside a pinned linux/amd64 container, emulated on other
# architectures, with exactly the `irp-components` job's toolchain, tools,
# flags, and command:
#
#   * image: the official Rust 1.98.0 Debian bookworm image, pinned by
#     digest and forced to linux/amd64;
#   * toolchain: rust-toolchain.toml's channel with the wasm32-wasip2 target;
#   * tools: wasm-tools 1.244.0 and wasi-virt at the pinned revision the
#     workflow installs;
#   * build: scripts/verify-wasm-capabilities.sh over the eight components
#     the conformance executor links, the example extension component
#     (components/scxml-extension-example) the namespace suites bind
#     (namespaces/<name>/irp), and the scx namespace's component
#     (namespaces/scx) and the GitHub resolver component
#     (components/scxml-resolver-github) that `scx` embeds with them, which sources
#     scripts/reproducible-wasm-env.sh
#     for the release rustflags.
#
# The working tree (not just committed files; build output and VCS metadata
# excluded) is snapshotted into the container, so the build reads the same
# sources the host sees. DIR receives the verified `.virtualized.wasm`
# artifacts and their `manifest.tsv`; the script prints the manifest's
# SHA-256, which is the `--components-manifest-sha256` value for the
# conformance CLI and the baseline generator, and the SHA-256 of the
# example extension component, which is their `--extension-component-sha256`
# value. Cargo's registry and target
# directory persist in named Docker volumes between runs; remove them with
# `docker volume rm scxml-canonical-cargo-registry scxml-canonical-target`.
set -eu

# Pinned base image: rust:1.98.0-bookworm (the manifest-list digest; the
# `--platform` flag selects its linux/amd64 image).
base_image='rust:1.98.0-bookworm@sha256:82150a52ec202c1b14d7817e14516c392bb7f5cfebd88f1ed531cb37ebd39922'
platform='linux/amd64'
tools_image='scxml-canonical-components:1'
wasm_tools_version='1.244.0'
wasi_virt_source='https://github.com/bytecodealliance/WASI-Virt'
wasi_virt_rev='448f6df8f688cee5d6995e96b1ffc31f9bf00742'
components='scxml_clock scxml_data_model_ecmascript scxml_data_model_null scxml_data_model_xpath scxml_ioprocessor_scxml scxml_ioprocessor_basichttp scxml_compiler scxml_machine scxml_extension_example scxml_resolver_github scx'

usage() {
    echo "usage: build-canonical-components.sh --artifacts-dir DIR" >&2
    exit 2
}

artifacts_dir=''
while [ "$#" -gt 0 ]; do
    case "$1" in
        --artifacts-dir)
            [ "$#" -ge 2 ] && [ -n "$2" ] || usage
            [ -z "$artifacts_dir" ] || usage
            artifacts_dir=$2
            shift 2
            ;;
        *) usage ;;
    esac
done
[ -n "$artifacts_dir" ] || usage

command -v docker >/dev/null 2>&1 || {
    echo "build-canonical-components: docker is required" >&2
    exit 1
}

workspace=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
. "$workspace/scripts/sha256.sh"
mkdir -p "$artifacts_dir"
artifacts_dir=$(CDPATH= cd -- "$artifacts_dir" && pwd -P)

channel=$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$workspace/rust-toolchain.toml")
[ "$channel" = '1.98.0' ] || {
    echo "build-canonical-components: rust-toolchain.toml channel is '$channel'; update the pinned base image with it" >&2
    exit 1
}

# The tools image: the pinned base plus the workflow's pinned tools. Docker
# caches it, so only the first run installs the tools.
docker build --platform "$platform" --tag "$tools_image" - <<EOF
FROM $base_image
RUN rustup toolchain install $channel --profile minimal --target wasm32-wasip2 --component clippy,rustfmt \
 && cargo +$channel install --locked wasm-tools@$wasm_tools_version \
 && cargo +$channel install --locked --git $wasi_virt_source --rev $wasi_virt_rev wasi-virt \
 && rm -rf /usr/local/cargo/registry /usr/local/cargo/git
EOF

snapshot=$(mktemp -d "${TMPDIR:-/tmp}/scxml-canonical-snapshot.XXXXXX")
trap 'rm -rf "$snapshot"' EXIT HUP INT TERM
(cd "$workspace" && tar -cf - --exclude=./target --exclude=./.git --exclude=./.claude --exclude=./.pi --exclude=./.commandcode --exclude=./handoff.md --exclude=node_modules --exclude='./hosts/scxml-host-browser/dist' .) \
    | (cd "$snapshot" && tar -xf -)

docker run --rm --platform "$platform" \
    --volume "$snapshot:/snapshot:ro" \
    --volume "$artifacts_dir:/artifacts" \
    --volume scxml-canonical-cargo-registry:/usr/local/cargo/registry \
    --volume scxml-canonical-target:/target \
    --env CARGO_TARGET_DIR=/target \
    "$tools_image" \
    sh -eu -c "
        mkdir -p /scxml.rs
        cp -R /snapshot/. /scxml.rs/
        cd /scxml.rs
        test \"\$(uname -m)\" = x86_64
        sh scripts/verify-wasm-capabilities.sh --artifacts-dir /artifacts $components
    "

manifest_sha256=$(sha256_file "$artifacts_dir/manifest.tsv")
extension_sha256=$(sha256_file "$artifacts_dir/scxml_extension_example.virtualized.wasm")
printf 'canonical-components dir=%s manifest-sha256=%s extension-sha256=%s\n' "$artifacts_dir" "$manifest_sha256" "$extension_sha256"
