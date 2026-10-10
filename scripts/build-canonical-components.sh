#!/bin/sh
# Builds floppy's canonical release components on any Docker host
# (docs/release.md), adapted from scxml.rs/scripts/build-canonical-components.sh.
#
# Usage: sh scripts/build-canonical-components.sh --artifacts-dir DIR
#
# The canonical identity is the Linux x86_64 build: release component bytes
# depend on the build host's target triple (decision D27; scxml.rs
# docs/conformance.md, "Canonical release identity"), so this script builds
# inside a pinned linux/amd64 container, emulated on other architectures:
#
#   * image: the official Rust 1.98.0 Debian bookworm image, pinned by
#     digest (scxml.rs's) and forced to linux/amd64;
#   * toolchain: rust-toolchain.toml's channel with the wasm32-wasip2 target;
#   * tools: wasm-tools 1.244.0, wasi-virt at the pinned revision, and
#     wasi-sdk 25.0 (the sql service's C toolchain), checked by SHA-256;
#   * build: scripts/build-components.sh, exactly as a contributor runs it
#     (the reproducible release flags of scripts/reproducible-wasm-env.sh,
#     wasi-virt twice, wasm-tools validation, the import allowlists).
#
# The working tree (build output excluded; its git metadata included, for
# the dev cache's commit) is snapshotted into the container, so the build
# reads the same sources the host sees. DIR receives what
# scripts/build-components.sh writes: DIR/scxml, DIR/namespaces,
# DIR/services (each with its manifest of sizes and SHA-256 digests), and
# DIR/package-cache.
#
# Then it checks that the build reproduces the digests the checked-in
# manifests pin for components: every service manifest's `validator-sha256`
# must be the SHA-256 of the package validator this build produced, the
# digest a release's SHA256SUMS lists (scripts/prepare-release.sh). A
# mismatch fails, naming the canonical digest to record.
#
# Cargo's registry and target directory persist in named Docker volumes
# between runs; remove them with
# `docker volume rm floppy-canonical-cargo-registry floppy-canonical-target`.
set -eu

# Pinned base image: rust:1.98.0-bookworm (the manifest-list digest; the
# `--platform` flag selects its linux/amd64 image), as scxml.rs pins it.
base_image='rust:1.98.0-bookworm@sha256:82150a52ec202c1b14d7817e14516c392bb7f5cfebd88f1ed531cb37ebd39922'
platform='linux/amd64'
tools_image='floppy-canonical-components:1'
wasm_tools_version='1.244.0'
wasi_virt_source='https://github.com/bytecodealliance/WASI-Virt'
wasi_virt_rev='448f6df8f688cee5d6995e96b1ffc31f9bf00742'
wasi_sdk_url='https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-25/wasi-sdk-25.0-x86_64-linux.tar.gz'
wasi_sdk_sha256='52640dde13599bf127a95499e61d6d640256119456d1af8897ab6725bcf3d89c'

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

workspace=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
. "$workspace/scripts/sha256.sh"
mkdir -p "$artifacts_dir"
artifacts_dir=$(CDPATH='' cd -- "$artifacts_dir" && pwd -P)

channel=$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$workspace/rust-toolchain.toml")
[ "$channel" = '1.98.0' ] || {
    echo "build-canonical-components: rust-toolchain.toml channel is '$channel'; update the pinned base image with it" >&2
    exit 1
}

# The tools image: the pinned base plus the pinned tools. Docker caches it,
# so only the first run installs them.
docker build --platform "$platform" --tag "$tools_image" - <<EOF
FROM $base_image
RUN rustup toolchain install $channel --profile minimal --target wasm32-wasip2 \
 && cargo +$channel install --locked wasm-tools@$wasm_tools_version \
 && cargo +$channel install --locked --git $wasi_virt_source --rev $wasi_virt_rev wasi-virt \
 && rm -rf /usr/local/cargo/registry /usr/local/cargo/git
RUN curl -fsSL -o /tmp/wasi-sdk.tar.gz $wasi_sdk_url \
 && echo '$wasi_sdk_sha256  /tmp/wasi-sdk.tar.gz' | sha256sum -c - \
 && mkdir -p /opt/wasi-sdk \
 && tar -xzf /tmp/wasi-sdk.tar.gz -C /opt/wasi-sdk --strip-components=1 \
 && rm /tmp/wasi-sdk.tar.gz
ENV WASI_SDK_PATH=/opt/wasi-sdk
EOF

snapshot=$(mktemp -d "${TMPDIR:-/tmp}/floppy-canonical-snapshot.XXXXXX")
trap 'rm -rf "$snapshot"' EXIT HUP INT TERM
(cd "$workspace" && tar -cf - --exclude=./target --exclude=./.claude --exclude=node_modules .) \
    | (cd "$snapshot" && tar -xf -)

# The target volume is the checkout's target/, where build-components.sh
# keeps the subtree's target directory (target/scxml.rs) too.
docker run --rm --platform "$platform" \
    --volume "$snapshot:/snapshot:ro" \
    --volume "$artifacts_dir:/artifacts" \
    --volume floppy-canonical-cargo-registry:/usr/local/cargo/registry \
    --volume floppy-canonical-target:/floppy/target \
    "$tools_image" \
    sh -eu -c "
        cp -R /snapshot/. /floppy/
        cd /floppy
        test \"\$(uname -m)\" = x86_64
        git config --global --add safe.directory /floppy
        sh scripts/build-components.sh --artifacts-dir /artifacts
    "

# The digests the checked-in manifests pin for components.
field() {
    sed -n "s/^  \"$1\": \"\\(.*\\)\",\\{0,1\\}\$/\\1/p" "$2"
}
status=0
for manifest in "$workspace"/services/*/package/*.service.json; do
    validator=$(field validator-artifact "$manifest")
    [ -n "$validator" ] || continue
    pinned=$(field validator-sha256 "$manifest")
    built=$(sha256_file "$artifacts_dir/services/$validator")
    if [ "$built" = "$pinned" ]; then
        printf 'reproduced %s sha256=%s\n' "$validator" "$built"
    else
        printf '%s: validator-sha256 is %s, the canonical build is %s: record it\n' \
            "${manifest#"$workspace"/}" "$pinned" "$built" >&2
        status=1
    fi
done
printf 'canonical-components dir=%s namespaces-sha256=%s services-sha256=%s\n' "$artifacts_dir" \
    "$(sha256_file "$artifacts_dir/namespaces/components.tsv")" \
    "$(sha256_file "$artifacts_dir/services/components.tsv")"
exit "$status"
