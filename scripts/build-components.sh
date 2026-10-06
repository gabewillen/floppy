#!/bin/sh
# Builds and verifies nemobot's Wasm components for wasm32-wasip2, the way
# scxml.rs builds its own (scxml.rs/scripts/verify-wasm-capabilities.sh).
#
# Usage: sh scripts/build-components.sh [--artifacts-dir DIR] [--session-only]
#
# 1. The scxml.rs session components a `scxml-cli run` session links
#    (machine, compiler, the three data models, both I/O processors, clock),
#    built and verified by the subtree's own
#    scxml.rs/scripts/verify-wasm-capabilities.sh into DIR/scxml, whose
#    manifest.tsv SHA-256 is the run's `--components-manifest-sha256`. Its
#    CARGO_TARGET_DIR is target/scxml.rs (never the subtree's own target
#    directory).
# 2. Unless --session-only: every nemobot namespace component and invoked
#    service component, built one cargo invocation per component (so
#    features never unify across them) with the reproducible release flags
#    (scripts/reproducible-wasm-env.sh), WASI virtualized by the pinned
#    wasi-virt (twice: the bytes must be identical; a component that imports
#    no WASI interface is used as built), validated with the pinned
#    wasm-tools, and its imports checked against its allowlist below (no
#    WASI interface is retained). DIR/namespaces and DIR/services receive
#    each `<component>.virtualized.wasm` and a `components.tsv` with their
#    sizes and SHA-256 digests.
# 3. Unless --session-only: DIR/components.args, a scxml-cli argument file
#    (`scxml-cli run @DIR/components.args ...`) that binds the session
#    components and every package above to its verified component by
#    SHA-256.
#
# DIR defaults to target/components. Tool identities are checked by the
# scxml.rs script; this script checks them again before building namespaces.
set -eu

expected_wasm_tools='wasm-tools 1.240.0'
expected_wasi_virt='wasi-virt 0.2.0'
expected_wasi_virt_source='https://github.com/bytecodealliance/WASI-Virt'
expected_wasi_virt_rev='448f6df8f688cee5d6995e96b1ffc31f9bf00742'
session_components='scxml_machine scxml_compiler scxml_data_model_ecmascript scxml_data_model_xpath scxml_data_model_null scxml_ioprocessor_scxml scxml_ioprocessor_basichttp scxml_clock'
namespace_components='nemobot_scxml nemobot_decision'
service_components='nemobot_service_generate nemobot_service_decision'

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

# Expected imports of each component after virtualization, one per line.
# Extension components (executable content) are synchronous: none imports
# outbound-http. Every model call is an invoked service.
expected_imports() {
    case "$1" in
        # <sx:validate>: reads its source through the data model and assigns
        # the report there; no execution-host, outbound-http, or snapshot.
        nemobot_scxml)
            printf '%s\n' \
                'stateforward:scxml/types@0.2.0' \
                'stateforward:scxml/telemetry@0.2.0' \
                'stateforward:scxml/data-model-host@0.2.0'
            ;;
        # <decision:menu>: reads the transitions through the session
        # snapshot, evaluates their guards through the data model, and
        # assigns the menu there; no execution-host or outbound-http.
        nemobot_decision)
            printf '%s\n' \
                'stateforward:scxml/types@0.2.0' \
                'stateforward:scxml/telemetry@0.2.0' \
                'stateforward:scxml/data-model-host@0.2.0' \
                'stateforward:scxml/session-snapshot-host@0.2.0'
            ;;
        # The generate and decision services: emit and complete through
        # invoked-service-host, one deferred request through outbound-http
        # (begin/take); nothing else.
        nemobot_service_generate|nemobot_service_decision)
            printf '%s\n' \
                'stateforward:scxml/types@0.2.0' \
                'stateforward:scxml/telemetry@0.2.0' \
                'stateforward:scxml/invoked-service-host@0.2.0' \
                'stateforward:scxml/outbound-http@0.2.0'
            ;;
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

# 2. nemobot namespace and service components.
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
run_tmp="$artifacts_dir/tmp"
rm -rf "$run_tmp"
mkdir -p "$run_tmp" "$artifacts_dir/namespaces" "$artifacts_dir/services"
# The pinned wasi-virt runs wasm-opt through a fixed temporary file name.
TMPDIR=$run_tmp
export TMPDIR
trap 'rm -rf "$run_tmp"; rm -f "$artifacts_dir"/namespaces/*.tmp "$artifacts_dir"/services/*.tmp "$artifacts_dir"/namespaces/*.determinism-check.wasm "$artifacts_dir"/services/*.determinism-check.wasm' EXIT HUP INT TERM

for component in $namespace_components $service_components; do
    package=$(printf '%s' "$component" | tr '_' '-')
    (cd "$workspace" && cargo build --locked --release --target wasm32-wasip2 -p "$package")
done

# Virtualizes, validates, and checks every component of one kind into
# DIR/<kind>, and writes its components.tsv.
build_kind() {
    kind_dir="$artifacts_dir/$1"
    shift
    manifest="$kind_dir/components.tsv"
    manifest_tmp="$manifest.tmp"
    {
        printf 'schema\tstateforward.nemobot.wasm-artifacts/v1\n'
        printf 'profile\trelease\n'
        printf 'target\twasm32-wasip2\n'
        printf 'wasm-tools\t%s\n' "$expected_wasm_tools"
        printf 'wasi-virt\t%s\n' "$expected_wasi_virt"
    } > "$manifest_tmp"
    for component in "$@"; do
        build_component "$component" "$kind_dir" "$manifest_tmp"
    done
    mv "$manifest_tmp" "$manifest"
    printf '%s-components path=%s sha256=%s\n' "$(basename "$kind_dir")" "$manifest" "$(sha256_file "$manifest")"
}

build_component() {
    component=$1
    kind_dir=$2
    manifest_tmp=$3
    raw="$release_dir/$component.wasm"
    virtualized="$kind_dir/$component.virtualized.wasm"
    repeat="$kind_dir/$component.determinism-check.wasm"
    test -f "$raw" || { echo "missing release component artifact: $raw" >&2; exit 1; }
    virt_input=$raw
    case "$component" in
        nemobot_scxml|nemobot_decision|nemobot_service_generate|nemobot_service_decision)
        # <sx:validate> links the scxml.rs validator, whose bundled XML
        # Schema backend seeds ahash from wasi:random/random and whose
        # `event` schema checks use std's RandomState (insecure-seed);
        # <decision:menu> compiles each offered schema, the generate service
        # checks structured output, and the decision service checks the
        # menu's schemas and the filled event data with the same `event`
        # schema code (boon, std's RandomState). wasi-virt denies randomness
        # (the first schema check would trap), so compose the subtree's
        # pinned deterministic provider first, exactly as scxml.rs does for
        # its validator component (see the .wat header); the values only
        # key in-memory hash tables.
        random="$run_tmp/wasi-random-validator.wasm"
        seeded="$run_tmp/$component.seeded.wasm"
        wasm-tools parse "$workspace/scxml.rs/components/scxml-validator/wasi-random.wat" -o "$random"
        wasm-tools compose "$raw" -d "$random" -o "$seeded" 2>/dev/null
        virt_input=$seeded
        ;;
    esac
    if wasm-tools component wit "$virt_input" | sed -n 's/^  import \(.*\);$/\1/p' | grep -q '^wasi:'; then
        wasi-virt "$virt_input" -o "$virtualized"
        wasi-virt "$virt_input" -o "$repeat"
    else
        # wasi-virt refuses a component with no WASI import to plug: there
        # is nothing to virtualize, so the release bytes are the artifact.
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
}

build_kind namespaces $namespace_components
build_kind services $service_components

# 3. DIR/components.args: a scxml-cli argument file (`scxml-cli run @FILE`,
# one argument per line) that binds the session components and every
# package above to the components just verified, each authenticated by its
# SHA-256 (the trust anchors scxml-cli requires; the manifests are not).
# Bots add their own argument file (bots/<name>/<name>.args) for the rest.
package_args() {
    flag=$1
    uri_field=$2
    manifest_path=$3
    component_path=$4
    uri=$(sed -n "s/.*\"$uri_field\": *\"\([^\"]*\)\".*/\1/p" "$manifest_path")
    [ -n "$uri" ] || { echo "no $uri_field in $manifest_path" >&2; exit 1; }
    printf '%s-package\n%s\n' "$flag" "$manifest_path"
    printf '%s-component\n%s=%s\n' "$flag" "$uri" "$component_path"
    printf '%s-component-sha256\n%s=%s\n' "$flag" "$uri" "$(sha256_file "$component_path")"
}
args_file="$artifacts_dir/components.args"
args_tmp="$args_file.tmp"
{
    printf -- '--components\n%s\n' "$artifacts_dir/scxml"
    printf -- '--components-manifest-sha256\n%s\n' "$session_manifest_sha256"
    for component in $namespace_components; do
        name=${component#nemobot_}
        package_args --extension namespace-uri \
            "$workspace/namespaces/$name/package/$name.extension.json" \
            "$artifacts_dir/namespaces/$component.virtualized.wasm"
    done
    for component in $service_components; do
        name=${component#nemobot_service_}
        package_args --service service-uri \
            "$workspace/services/$name/package/$name.service.json" \
            "$artifacts_dir/services/$component.virtualized.wasm"
    done
} > "$args_tmp"
mv "$args_tmp" "$args_file"
printf 'components-args path=%s\n' "$args_file"
