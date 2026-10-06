#!/bin/sh
# Builds and verifies nemobot's Wasm components for wasm32-wasip2, the way
# scxml.rs builds its own (scxml.rs/scripts/verify-wasm-capabilities.sh).
#
# Usage: sh scripts/build-components.sh [--artifacts-dir DIR] [--session-only]
#
# 1. The scxml.rs session components a nemobot-host session links (machine,
#    compiler, the three data models, both I/O processors, clock), built and
#    verified by the subtree's own scxml.rs/scripts/verify-wasm-capabilities.sh
#    into DIR/scxml, whose manifest.tsv SHA-256 is the config's
#    `components.manifest-sha256`. Its CARGO_TARGET_DIR is
#    target/scxml.rs (never the subtree's own target directory).
# 2. Unless --session-only: every nemobot namespace component, built one
#    cargo invocation per component (so features never unify across them)
#    with the reproducible release flags (scripts/reproducible-wasm-env.sh),
#    WASI virtualized by the pinned wasi-virt (twice: the bytes must be
#    identical; a component that imports no WASI interface is used as
#    built), validated with the pinned wasm-tools, and its imports checked
#    against its allowlist below (no WASI interface is retained). DIR receives
#    each `<component>.virtualized.wasm` and `components.tsv` with their sizes
#    and SHA-256 digests (the `component-sha256` of each package in a config).
#
# DIR defaults to target/components. Tool identities are checked by the
# scxml.rs script; this script checks them again before building namespaces.
set -eu

expected_wasm_tools='wasm-tools 1.240.0'
expected_wasi_virt='wasi-virt 0.2.0'
expected_wasi_virt_source='https://github.com/bytecodealliance/WASI-Virt'
expected_wasi_virt_rev='448f6df8f688cee5d6995e96b1ffc31f9bf00742'
session_components='scxml_machine scxml_compiler scxml_data_model_ecmascript scxml_data_model_xpath scxml_data_model_null scxml_ioprocessor_scxml scxml_ioprocessor_basichttp scxml_clock'
namespace_components='nemobot_scxml nemobot_generate nemobot_decision'

usage() {
    echo "usage: build-components.sh [--artifacts-dir DIR] [--session-only]" >&2
    exit 2
}

artifacts_dir=''
session_only=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        --artifacts-dir)
            [ "$#" -ge 2 ] && [ -n "$2" ] || usage
            [ -z "$artifacts_dir" ] || usage
            artifacts_dir=$2
            shift 2
            ;;
        --session-only)
            session_only=true
            shift
            ;;
        *) usage ;;
    esac
done

workspace=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
. "$workspace/scripts/sha256.sh"
[ -n "$artifacts_dir" ] || artifacts_dir="$workspace/target/components"
case "$artifacts_dir" in
    /*) ;;
    *) artifacts_dir="$(pwd -P)/$artifacts_dir" ;;
esac
mkdir -p "$artifacts_dir"

# Expected imports of each namespace component after virtualization, one per
# line. The skeletons import nothing yet; each namespace adds its exact
# stateforward:scxml interfaces here when it implements its component.
expected_imports() {
    case "$1" in
        nemobot_scxml|nemobot_generate|nemobot_decision) ;;
        *) echo "no import allowlist for component: $1" >&2; exit 1 ;;
    esac
}

# 1. Session components, through the subtree's own proof script.
(
    cd "$workspace/scxml.rs"
    unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_TARGET_WASM32_WASIP2_RUSTFLAGS
    CARGO_TARGET_DIR="$workspace/target/scxml.rs" \
        sh scripts/verify-wasm-capabilities.sh --artifacts-dir "$artifacts_dir/scxml" $session_components
)
session_manifest_sha256=$(sha256_file "$artifacts_dir/scxml/manifest.tsv")
printf 'session-components dir=%s manifest-sha256=%s\n' "$artifacts_dir/scxml" "$session_manifest_sha256"
[ "$session_only" = false ] || exit 0

# 2. nemobot namespace components.
actual_wasm_tools=$(wasm-tools --version | cut -d ' ' -f 1-2)
test "$actual_wasm_tools" = "$expected_wasm_tools" || {
    echo "wasm-tools identity mismatch: expected $expected_wasm_tools, found $actual_wasm_tools" >&2
    exit 1
}
actual_wasi_virt=$(wasi-virt --version | cut -d ' ' -f 1-2)
test "$actual_wasi_virt" = "$expected_wasi_virt" || {
    echo "wasi-virt identity mismatch: expected $expected_wasi_virt, found $actual_wasi_virt" >&2
    exit 1
}
case "$(cargo install --list | grep '^wasi-virt ' || true)" in
    "wasi-virt v0.2.0 ($expected_wasi_virt_source?rev=$expected_wasi_virt_rev#"*"):") ;;
    *)
        echo "wasi-virt revision mismatch: install it with: cargo install --locked --git $expected_wasi_virt_source --rev $expected_wasi_virt_rev wasi-virt" >&2
        exit 1
        ;;
esac

unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_TARGET_WASM32_WASIP2_RUSTFLAGS
. "$workspace/scripts/reproducible-wasm-env.sh"
target_root=${CARGO_TARGET_DIR:-$workspace/target}
case "$target_root" in
    /*) ;;
    *) target_root="$workspace/$target_root" ;;
esac
release_dir="$target_root/wasm32-wasip2/release"
namespaces_dir="$artifacts_dir/namespaces"
mkdir -p "$namespaces_dir"
run_tmp="$namespaces_dir/tmp"
rm -rf "$run_tmp"
mkdir -p "$run_tmp"
# The pinned wasi-virt runs wasm-opt through a fixed temporary file name.
TMPDIR=$run_tmp
export TMPDIR
manifest="$namespaces_dir/components.tsv"
manifest_tmp="$manifest.tmp"
trap 'rm -rf "$run_tmp"; rm -f "$manifest_tmp" "$namespaces_dir"/*.determinism-check.wasm' EXIT HUP INT TERM

for component in $namespace_components; do
    package=$(printf '%s' "$component" | tr '_' '-')
    (cd "$workspace" && cargo build --locked --release --target wasm32-wasip2 -p "$package")
done

{
    printf 'schema\tstateforward.nemobot.wasm-artifacts/v1\n'
    printf 'profile\trelease\n'
    printf 'target\twasm32-wasip2\n'
    printf 'wasm-tools\t%s\n' "$expected_wasm_tools"
    printf 'wasi-virt\t%s\n' "$expected_wasi_virt"
} > "$manifest_tmp"

for component in $namespace_components; do
    raw="$release_dir/$component.wasm"
    virtualized="$namespaces_dir/$component.virtualized.wasm"
    repeat="$namespaces_dir/$component.determinism-check.wasm"
    test -f "$raw" || { echo "missing release component artifact: $raw" >&2; exit 1; }
    if wasm-tools component wit "$raw" | sed -n 's/^  import \(.*\);$/\1/p' | grep -q '^wasi:'; then
        wasi-virt "$raw" -o "$virtualized"
        wasi-virt "$raw" -o "$repeat"
    else
        # wasi-virt refuses a component with no WASI import to plug (the
        # empty skeletons): there is nothing to virtualize, so the release
        # bytes are the artifact.
        cp "$raw" "$virtualized"
        cp "$raw" "$repeat"
    fi
    cmp -s "$virtualized" "$repeat" || { echo "non-deterministic wasi-virt output: $component" >&2; exit 1; }
    wasm-tools validate --features component-model "$virtualized"
    imports=$(wasm-tools component wit "$virtualized" | sed -n 's/^  import \(.*\);$/\1/p')
    if printf '%s\n' "$imports" | grep -q '^wasi:'; then
        echo "virtualized component retains a WASI capability: $component" >&2
        printf '%s\n' "$imports" >&2
        exit 1
    fi
    expected=$(expected_imports "$component")
    test "$imports" = "$expected" || {
        echo "component import allowlist mismatch: $component" >&2
        printf '%s\n' "$imports" >&2
        exit 1
    }
    size=$(wc -c < "$virtualized" | tr -d ' ')
    digest=$(sha256_file "$virtualized")
    printf 'artifact\t%s\t%s.virtualized.wasm\t%s\t%s\n' "$component" "$component" "$size" "$digest" >> "$manifest_tmp"
    printf 'verified %s sha256=%s bytes=%s\n' "$component" "$digest" "$size"
done

mv "$manifest_tmp" "$manifest"
printf 'namespace-components path=%s sha256=%s\n' "$manifest" "$(sha256_file "$manifest")"
