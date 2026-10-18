#!/bin/sh
# Builds and verifies floppy's Wasm components for wasm32-wasip2, the way
# scxml.rs builds its own (scxml.rs/scripts/verify-wasm-capabilities.sh).
#
# Usage: sh scripts/build-components.sh [--artifacts-dir DIR] [--session-only]
#
# 1. The scxml.rs session components a `scx run` session links (machine,
#    compiler, the three data models, both I/O processors, clock), the
#    `scx` namespace's component (`<scx:validate>`), and the GitHub resolver
#    component (repository packages), which `scx` embeds too, built and
#    verified by the subtree's own scxml.rs/scripts/verify-wasm-capabilities.sh
#    into DIR/scxml: the release `scx` embeds when it is built with
#    SCXML_WASM_ARTIFACTS_DIR=DIR/scxml (README.md, "Running a bot"). Its
#    CARGO_TARGET_DIR is target/scxml.rs (never the subtree's own target
#    directory).
# 2. Unless --session-only: every floppy namespace component, invoked
#    service component, and package validator component (DIR/services),
#    built one cargo invocation per component (so
#    features never unify across them) with the reproducible release flags
#    (scripts/reproducible-wasm-env.sh), WASI virtualized by the pinned
#    wasi-virt (twice: the bytes must be identical; a component that imports
#    no WASI interface is used as built), validated with the pinned
#    wasm-tools, and its imports checked against its allowlist below (no
#    WASI interface is retained). DIR/namespaces and DIR/services receive
#    each `<component>.virtualized.wasm` and a `components.tsv` with their
#    sizes and SHA-256 digests.
# 3. Unless --session-only: publishes every package above into a dev
#    repository-package cache, DIR/package-cache, exactly as `scx`'s
#    resolver stores a release it fetched: the manifest, schema, and
#    verified component bytes content-addressed under sha256/<hex>, and the
#    URI pinned in index/<sha256 of the URI>.json to this checkout's commit,
#    the release tag v<package-version> (a tagless URI's entry records the
#    release it was pinned to), and those digests. The documents' tagless
#    repository URIs (https://github.com/stateforward/floppy/namespaces/<name>
#    and .../services/<name>) then resolve from it with
#    `scx --offline --cache-dir DIR/package-cache`, with no network, and
#    every run re-verifies the bytes against the pins; the same documents
#    resolve the latest GitHub release without those flags. The dev cache
#    is never the release cache (`${XDG_CACHE_HOME:-$HOME/.cache}/scxml/
#    extensions`, which only `scx`'s resolver writes), so a dev build never
#    overwrites a release's entries.
#
# DIR defaults to target/components. Tool identities are checked by the
# scxml.rs script; this script checks them again before building namespaces.
set -eu

expected_wasm_tools='wasm-tools 1.244.0'
expected_wasi_virt='wasi-virt 0.2.0'
expected_wasi_virt_source='https://github.com/bytecodealliance/WASI-Virt'
expected_wasi_virt_rev='448f6df8f688cee5d6995e96b1ffc31f9bf00742'
# The C toolchain of the sql service's SQLite (libsqlite3-sys compiles
# the amalgamation with $WASI_SDK_PATH/bin/clang and its sysroot): the
# pinned wasi-sdk release, checked by its VERSION file.
expected_wasi_sdk='25.0'
session_components='scxml_machine scxml_compiler scxml_data_model_ecmascript scxml_data_model_xpath scxml_data_model_null scxml_ioprocessor_scxml scxml_ioprocessor_basichttp scxml_clock scxml_resolver_github scx'
namespace_components='floppy_decision floppy_conversation floppy_migration floppy_device floppy_processing floppy_intuition floppy_cognition floppy_identity floppy_memory floppy_behavior floppy_autonomy'
service_components='floppy_service_generate floppy_service_decision floppy_service_sql floppy_service_speech floppy_service_pgsql'
# Package validator components (stateforward:scxml-validator/package-validator),
# each named by its package's manifest (`validator-artifact`).
validator_components='floppy_service_generate_validator'

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
        # <decision:choice>: reads the transitions through the session
        # snapshot, evaluates their guards through the data model, and
        # assigns the choices there; no execution-host or outbound-http.
        floppy_decision)
            printf '%s\n' \
                'stateforward:scxml/types@0.2.0' \
                'stateforward:scxml/telemetry@0.2.0' \
                'stateforward:scxml/data-model-host@0.2.0' \
                'stateforward:scxml/session-snapshot-host@0.2.0'
            ;;
        # <device:snapshot>: reads another session of the run through the
        # session snapshot (its target) and assigns the read through the
        # data model; no execution-host or outbound-http.
        floppy_device)
            printf '%s\n' \
                'stateforward:scxml/types@0.2.0' \
                'stateforward:scxml/telemetry@0.2.0' \
                'stateforward:scxml/data-model-host@0.2.0' \
                'stateforward:scxml/session-snapshot-host@0.2.0'
            ;;
        # The conversation elements (<conversation:message_for_turn>,
        # <conversation:relationship_for_input> and the rest of the
        # original's conversation module and turn detector): evaluate their
        # expressions and assign their results through the data model; no
        # session snapshot, execution-host, or outbound-http.
        # <migration:plan>: evaluates its expressions and assigns the plan
        # through the data model; no session snapshot, execution-host, or
        # outbound-http.
        # The processing elements (<processing:model_facing_schema>,
        # <processing:collect_offered_events>, <processing:send>,
        # <processing:selection_confidence>): evaluate their expressions and
        # assign their results through the data model; no session snapshot,
        # execution-host, or outbound-http.
        # The intuition elements (<intuition:observe>, <intuition:escalate>),
        # the cognition elements (<cognition:attention_bias_for_turn>,
        # <cognition:model_facing_content>), the identity elements
        # (<identity:hears_name>, <identity:sorted_identities>) and the
        # memory elements (<memory:to_json>, <memory:contents>), the
        # behavior elements (<behavior:instances_from_behavior_results>,
        # <behavior:replace_behavior_clauses>, <behavior:lifetime_errors>,
        # <behavior:verify_tick>) and the autonomy element
        # (<autonomy:behavior_input_payload>): the same.
        floppy_migration|floppy_conversation|floppy_processing|floppy_intuition|floppy_cognition|floppy_identity|floppy_memory|floppy_behavior|floppy_autonomy)
            printf '%s\n' \
                'stateforward:scxml/types@0.2.0' \
                'stateforward:scxml/telemetry@0.2.0' \
                'stateforward:scxml/data-model-host@0.2.0'
            ;;
        # The decision service: one async `invoke` export (its events go to
        # the stream it returns) and one request through outbound-http's
        # async `fetch`; it never reads the logical clock.
        floppy_service_decision)
            printf '%s\n' \
                'stateforward:scxml/types@0.2.0' \
                'stateforward:scxml/outbound-http@0.2.0' \
                'stateforward:scxml/telemetry@0.2.0'
            ;;
        # The generate service: the same, and the images a message names,
        # read from the directory the run grants. Like the sql service it
        # keeps exactly the filesystem it imports for the host (linked only
        # under `--dir`); wasi:cli and the 0.2 monotonic clock are denied by
        # the composed provider (services/sql/wasi-cli-deny.wat). It reads
        # the world's 0.3 monotonic clock (the session's logical clock) to
        # space a streamed call's progress events (decision D101).
        floppy_service_generate)
            printf '%s\n' \
                'wasi:clocks/monotonic-clock@0.3.0' \
                'stateforward:scxml/types@0.2.0' \
                'stateforward:scxml/outbound-http@0.2.0' \
                'stateforward:scxml/telemetry@0.2.0' \
                'wasi:io/poll@0.2.12' \
                'wasi:clocks/wall-clock@0.2.12' \
                'wasi:io/error@0.2.12' \
                'wasi:io/streams@0.2.12' \
                'wasi:filesystem/types@0.2.12' \
                'wasi:filesystem/preopens@0.2.12'
            ;;
        # The sql service: one async `invoke` export and SQLite on a file
        # in the directory the run grants. It keeps exactly the filesystem
        # it imports for the host (which links it only under `--dir`): the
        # filesystem interfaces, the wasi:io interfaces they use, and
        # wasi:clocks/wall-clock for SQLite's CURRENT_TIMESTAMP; wasi:cli
        # and the 0.2 monotonic clock are denied by the composed provider
        # (services/sql/wasi-cli-deny.wat).
        floppy_service_sql)
            printf '%s\n' \
                'stateforward:scxml/types@0.2.0' \
                'stateforward:scxml/telemetry@0.2.0' \
                'wasi:io/poll@0.2.12' \
                'wasi:clocks/wall-clock@0.2.12' \
                'wasi:io/error@0.2.12' \
                'wasi:io/streams@0.2.12' \
                'wasi:filesystem/types@0.2.12' \
                'wasi:filesystem/preopens@0.2.12'
            ;;
        # The speech service: one async `invoke` export, one request through
        # outbound-http's async `fetch`, and the media files in the directory
        # the run grants. It keeps exactly the filesystem it imports for the
        # host (which links it only under `--dir`): the filesystem
        # interfaces, the wasi:io interfaces they use, and
        # wasi:clocks/wall-clock for their timestamps; wasi:cli and the 0.2
        # monotonic clock are denied by the composed provider
        # (services/speech/wasi-cli-deny.wat).
        floppy_service_speech)
            printf '%s\n' \
                'stateforward:scxml/types@0.2.0' \
                'stateforward:scxml/outbound-http@0.2.0' \
                'stateforward:scxml/telemetry@0.2.0' \
                'wasi:io/poll@0.2.12' \
                'wasi:clocks/wall-clock@0.2.12' \
                'wasi:io/error@0.2.12' \
                'wasi:io/streams@0.2.12' \
                'wasi:filesystem/types@0.2.12' \
                'wasi:filesystem/preopens@0.2.12'
            ;;
        # The pgsql service: one async `invoke` export and the Postgres wire
        # protocol on a TCP connection the run grants. It keeps exactly the
        # sockets it imports for the host (which links them only under
        # `--wasi URI=inherit-network`) and the wasi:io interfaces its
        # streams use; wasi:cli and the monotonic clock are denied by the
        # composed provider (services/pgsql/wasi-cli-deny.wat).
        floppy_service_pgsql)
            printf '%s\n' \
                'stateforward:scxml/types@0.2.0' \
                'stateforward:scxml/telemetry@0.2.0' \
                'wasi:io/poll@0.2.12' \
                'wasi:io/error@0.2.12' \
                'wasi:io/streams@0.2.12' \
                'wasi:sockets/network@0.2.12' \
                'wasi:sockets/instance-network@0.2.12' \
                'wasi:sockets/tcp@0.2.12' \
                'wasi:sockets/tcp-create-socket@0.2.12'
            ;;
        # A package validator's world imports nothing: its export's record
        # types arrive through the types-only types and document interfaces.
        floppy_service_generate_validator)
            printf '%s\n' \
                'stateforward:scxml-validator/types@0.2.0' \
                'stateforward:scxml-validator/document@0.2.0'
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

# 2. floppy namespace and service components.
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

[ -n "${WASI_SDK_PATH:-}" ] || {
    echo "WASI_SDK_PATH is not set: install wasi-sdk $expected_wasi_sdk (https://github.com/WebAssembly/wasi-sdk/releases/tag/wasi-sdk-25) and point WASI_SDK_PATH at it" >&2
    exit 1
}
actual_wasi_sdk=$(sed -n '1p' "$WASI_SDK_PATH/VERSION" 2>/dev/null || true)
test "$actual_wasi_sdk" = "$expected_wasi_sdk" || {
    echo "wasi-sdk identity mismatch: expected $expected_wasi_sdk, found ${actual_wasi_sdk:-none} at $WASI_SDK_PATH" >&2
    exit 1
}
export WASI_SDK_PATH

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

for component in $namespace_components $service_components $validator_components; do
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
        printf 'schema\tstateforward.floppy.wasm-artifacts/v1\n'
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
        floppy_decision|floppy_service_generate|floppy_service_decision|floppy_service_generate_validator)
        # <decision:choice> compiles each offered schema, the generate service
        # checks structured output, and the decision service checks the
        # choices' schemas and the filled event data with the same `event`
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
    if [ "$component" = floppy_service_sql ] || [ "$component" = floppy_service_speech ] || [ "$component" = floppy_service_generate ] || [ "$component" = floppy_service_pgsql ]; then
        # The sql, speech and generate services keep their filesystem
        # imports for the host (and pgsql its sockets): a wasi-virt adapter would virtualize the
        # filesystem too, so the standard library's and wasi-libc's other
        # WASI imports (wasi:cli and the monotonic clock) are satisfied by
        # a pinned deny-all provider instead (see the .wat header; generate
        # uses sql's), as scxml.rs does for its files example service.
        name=${component#floppy_service_}
        wat="$workspace/services/$name/wasi-cli-deny.wat"
        [ -f "$wat" ] || wat="$workspace/services/sql/wasi-cli-deny.wat"
        deny="$run_tmp/wasi-cli-deny-$name.wasm"
        wasm-tools parse "$wat" -o "$deny"
        wasm-tools compose "$virt_input" -d "$deny" -o "$virtualized" >/dev/null 2>&1
        wasm-tools compose "$virt_input" -d "$deny" -o "$repeat" >/dev/null 2>&1
    elif wasm-tools component wit "$virt_input" | sed -n 's/^  import \(.*\);$/\1/p' | grep -q '^wasi:'; then
        wasi-virt "$virt_input" -o "$virtualized"
        wasi-virt "$virt_input" -o "$repeat"
    else
        # wasi-virt refuses a component with no WASI import to plug: there
        # is nothing to virtualize, so the release bytes are the artifact.
        cp "$raw" "$virtualized"
        cp "$raw" "$repeat"
    fi
    cmp -s "$virtualized" "$repeat" || { echo "non-deterministic wasi-virt output: $component" >&2; exit 1; }
    wasm-tools validate --features component-model,cm-async "$virtualized"
    imports=$(wasm-tools component wit "$virtualized" | sed -n 's/^  import \(.*\);$/\1/p')
    # A component may keep only a WASI interface its allowlist names (the
    # sql and speech services the filesystem they import).
    allowed_wasi=$(expected_imports "$component" | grep -E '^wasi:' || true)
    if printf '%s\n' "$imports" | grep -E '^wasi:' | grep -Fxv "$allowed_wasi" | grep -q .; then
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
build_kind services $service_components $validator_components

# 3. Publish every package into the dev repository-package cache (scxml.rs
# crates/scxml-cli/src/repository.rs, "Trust and the cache": objects under
# sha256/<hex>, the pinned entry under index/<sha256 of the URI>.json in its
# canonical form). The commit is this checkout's.
cache_dir="$artifacts_dir/package-cache"
commit=$(git -C "$workspace" rev-parse HEAD)
case "$commit" in
    *[!0-9a-f]*|'') echo "not a git commit: $commit" >&2; exit 1 ;;
esac
[ "${#commit}" -eq 40 ] || { echo "not a 40-hex git commit: $commit" >&2; exit 1; }
mkdir -p "$cache_dir/sha256" "$cache_dir/index"

# Stores PATH content-addressed and prints its digest.
publish_object() {
    digest=$(sha256_file "$1")
    if [ ! -f "$cache_dir/sha256/$digest" ]; then
        cp "$1" "$cache_dir/sha256/$digest.tmp"
        mv "$cache_dir/sha256/$digest.tmp" "$cache_dir/sha256/$digest"
    fi
    printf '%s\n' "$digest"
}

# Prints the string value of FIELD in the manifest at PATH ("" for null).
manifest_field() {
    sed -n "s/.*\"$2\": *\"\([^\"]*\)\".*/\1/p" "$1"
}

# Writes the pinned entry of URI (canonical bytes) atomically.
publish_entry() {
    uri=$1
    entry_tmp="$cache_dir/index/entry.tmp"
    printf '%s' "$uri" > "$entry_tmp"
    key=$(sha256_file "$entry_tmp")
    cat > "$entry_tmp"
    mv "$entry_tmp" "$cache_dir/index/$key.json"
}

for component in $namespace_components; do
    name=${component#floppy_}
    manifest="$workspace/namespaces/$name/package/$name.extension.json"
    uri=$(manifest_field "$manifest" namespace-uri)
    schema="$workspace/namespaces/$name/package/$(manifest_field "$manifest" schema-artifact)"
    [ -n "$uri" ] && [ -f "$schema" ] || { echo "malformed manifest: $manifest" >&2; exit 1; }
    manifest_sha256=$(publish_object "$manifest")
    schema_sha256=$(publish_object "$schema")
    test "$schema_sha256" = "$(manifest_field "$manifest" schema-sha256)" || {
        echo "schema digest mismatch: $manifest" >&2
        exit 1
    }
    component_sha256=$(publish_object "$artifacts_dir/namespaces/$component.virtualized.wasm")
    tag="v$(manifest_field "$manifest" package-version)"
    printf '{\n  "format": "stateforward.scxml.extension-cache/v1",\n  "namespace-uri": "%s",\n  "tag": "%s",\n  "commit": "%s",\n  "manifest-sha256": "%s",\n  "schema-sha256": "%s",\n  "component-sha256": "%s"\n}\n' \
        "$uri" "$tag" "$commit" "$manifest_sha256" "$schema_sha256" "$component_sha256" | publish_entry "$uri"
    printf 'published %s component-sha256=%s\n' "$uri" "$component_sha256"
done
for component in $service_components; do
    name=${component#floppy_service_}
    manifest="$workspace/services/$name/package/$name.service.json"
    uri=$(manifest_field "$manifest" service-uri)
    [ -n "$uri" ] || { echo "malformed manifest: $manifest" >&2; exit 1; }
    schema_artifact=$(manifest_field "$manifest" schema-artifact)
    if [ -n "$schema_artifact" ]; then
        publish_object "$workspace/services/$name/package/$schema_artifact" > /dev/null
    fi
    # A package validator (`validator-artifact`, the component built above)
    # is published by its digest. Its
    # bytes depend on the build host (Cargo's symbol-mangling hash reads the
    # host's target triple), so the manifest the dev cache pins names this
    # build's digest: the checked-in one when they agree, else a copy with
    # `validator-sha256` set to it.
    validator_artifact=$(manifest_field "$manifest" validator-artifact)
    if [ -n "$validator_artifact" ]; then
        validator_sha256=$(publish_object "$artifacts_dir/services/$validator_artifact")
        if [ "$validator_sha256" != "$(manifest_field "$manifest" validator-sha256)" ]; then
            sed "s/\"validator-sha256\": *\"[0-9a-f]*\"/\"validator-sha256\": \"$validator_sha256\"/" "$manifest" > "$run_tmp/$name.service.json"
            manifest="$run_tmp/$name.service.json"
            printf 'note: %s pins this build'"'"'s package validator %s\n' "$uri" "$validator_sha256"
        fi
    fi
    manifest_sha256=$(publish_object "$manifest")
    component_sha256=$(publish_object "$artifacts_dir/services/$component.virtualized.wasm")
    tag="v$(manifest_field "$manifest" package-version)"
    printf '{\n  "format": "stateforward.scxml.service-cache/v1",\n  "service-uri": "%s",\n  "tag": "%s",\n  "commit": "%s",\n  "manifest-sha256": "%s",\n  "component-sha256": "%s"\n}\n' \
        "$uri" "$tag" "$commit" "$manifest_sha256" "$component_sha256" | publish_entry "$uri"
    printf 'published %s component-sha256=%s\n' "$uri" "$component_sha256"
done
printf 'package-cache path=%s commit=%s\n' "$cache_dir" "$commit"
