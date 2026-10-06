#!/bin/sh
set -eu

# The release whose wasmparser (0.244) matches wasmtime 49 and wit-bindgen
# 0.51: it reads the Component Model async ABI (WASI 0.3) that invoked
# services export and `outbound-http.fetch` uses.
expected_wasm_tools='wasm-tools 1.244.0'
expected_wasi_virt='wasi-virt 0.2.0'

# Compare only `<name> <version>`: prebuilt release binaries append
# ` (<commit> <date>)`, which does not change the tool's identity.
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
# Several upstream WASI-Virt commits report the same "0.2.0" version, and the
# checked-in IRP baselines bind the exact component bytes wasi-virt emits, so
# the installed wasi-virt must also be the pinned git revision. `cargo install
# --list` records that revision, and the wasi-virt on PATH must be the binary
# that record describes.
expected_wasi_virt_source='https://github.com/bytecodealliance/WASI-Virt'
expected_wasi_virt_rev='448f6df8f688cee5d6995e96b1ffc31f9bf00742'
wasi_virt_install_hint="install it with: cargo install --locked --git $expected_wasi_virt_source --rev $expected_wasi_virt_rev wasi-virt"
wasi_virt_installed=$(cargo install --list | grep '^wasi-virt ' || true)
case "$wasi_virt_installed" in
    "wasi-virt v0.2.0 ($expected_wasi_virt_source?rev=$expected_wasi_virt_rev#"*"):") ;;
    *)
        echo "wasi-virt revision mismatch: expected revision $expected_wasi_virt_rev of $expected_wasi_virt_source, found: ${wasi_virt_installed:-no cargo-installed wasi-virt}" >&2
        echo "$wasi_virt_install_hint" >&2
        exit 1
        ;;
esac
wasi_virt_cargo_bin="${CARGO_HOME:-${HOME:?HOME must be set}/.cargo}/bin/wasi-virt"
wasi_virt_on_path=$(command -v wasi-virt)
test "$wasi_virt_on_path" -ef "$wasi_virt_cargo_bin" || {
    echo "wasi-virt on PATH ($wasi_virt_on_path) is not the cargo-installed pinned revision at $wasi_virt_cargo_bin" >&2
    echo "$wasi_virt_install_hint" >&2
    exit 1
}

workspace=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
. "$workspace/scripts/sha256.sh"
# Release guest bytes (and therefore every digest below) must not depend on
# the checkout path, target directory, CARGO_HOME, or rust-src presence.
. "$workspace/scripts/reproducible-wasm-env.sh"
target_root=${CARGO_TARGET_DIR:-target}
case "$target_root" in
    /*) ;;
    *) target_root="$workspace/$target_root" ;;
esac
release_dir="$target_root/wasm32-wasip2/release"
# Usage: verify-wasm-capabilities.sh [--artifacts-dir DIR] [COMPONENT...]
#
# --artifacts-dir DIR names this run's own artifact directory. When the flag
# is absent, SCXML_WASM_ARTIFACTS_DIR is the documented fallback (the Rust
# test harnesses that build components set it), and without either the
# directory is $CARGO_TARGET_DIR/wasm-artifacts/release. Callers that share
# one CARGO_TARGET_DIR (Cargo serializes the builds themselves) MUST give each
# concurrent run a distinct directory: two runs writing one directory can
# publish a manifest whose component set or digests the other run did not
# verify, which the host then rejects as a digest mismatch.
artifacts_dir_flag=''
requested_components=''
while [ "$#" -gt 0 ]; do
    case "$1" in
        --artifacts-dir)
            if [ "$#" -lt 2 ] || [ -z "$2" ]; then
                echo "--artifacts-dir requires a directory" >&2
                exit 2
            fi
            if [ -n "$artifacts_dir_flag" ]; then
                echo "--artifacts-dir given more than once" >&2
                exit 2
            fi
            artifacts_dir_flag=$2
            shift 2
            ;;
        -*)
            echo "unknown option: $1" >&2
            echo "usage: verify-wasm-capabilities.sh [--artifacts-dir DIR] [COMPONENT...]" >&2
            exit 2
            ;;
        ''|*[!a-z0-9_]*)
            echo "unknown component request: $1" >&2
            exit 1
            ;;
        *)
            requested_components="${requested_components}${requested_components:+ }$1"
            shift
            ;;
    esac
done
# Every requested name matched [a-z0-9_]+ above, so the list round-trips
# through word splitting unchanged; globbing is off while it is re-split.
set -f
# shellcheck disable=SC2086
set -- $requested_components
set +f
if [ -n "$artifacts_dir_flag" ]; then
    proof_dir=$artifacts_dir_flag
else
    proof_dir=${SCXML_WASM_ARTIFACTS_DIR:-$target_root/wasm-artifacts/release}
fi
case "$proof_dir" in
    /*) ;;
    *) proof_dir="$workspace/$proof_dir" ;;
esac
manifest="$proof_dir/manifest.tsv"
manifest_tmp="$proof_dir/manifest.tsv.tmp"
all_components='scxml_clock scxml_data_model_ecmascript scxml_data_model_null scxml_data_model_xpath scxml_ioprocessor_scxml scxml_ioprocessor_basichttp scxml_service_example scxml_service_files scxml_extension_example scxml_script_example scxml_validator scxml_reporter_rustc scxml_reporter_sarif scxml_compiler scxml_machine scxml_snapshot_codec'
if [ "$#" -eq 0 ]; then
    components=$all_components
else
    requested=" $* "
    seen=' '
    for component in "$@"; do
        case "$seen" in
            *" $component "*) echo "duplicate component request: $component" >&2; exit 1 ;;
        esac
        seen="$seen$component "
        case " $all_components " in
            *" $component "*) ;;
            *) echo "unknown component request: $component" >&2; exit 1 ;;
        esac
    done
    components=''
    for component in $all_components; do
        case "$requested" in
            *" $component "*) components="${components}${components:+ }$component" ;;
        esac
    done
fi

mkdir -p "$proof_dir"
packaged_dir="$proof_dir/packaged"
# The pinned wasi-virt runs wasm-opt through a fixed temporary file name, so
# two concurrent runs sharing one temporary directory fail with a missing
# file. Every run owns a temporary directory inside its artifact directory.
run_tmp="$proof_dir/tmp"
rm -rf "$run_tmp"
mkdir -p "$run_tmp"
TMPDIR=$run_tmp
export TMPDIR
trap 'rm -rf "$run_tmp"; rm -f "$manifest_tmp" "$proof_dir"/*.determinism-check.wasm "$packaged_dir"/*.determinism-check.*' EXIT HUP INT TERM

expected_imports() {
    case "$1" in
        scxml_clock) printf '%s\n' 'stateforward:scxml-clock/types@0.2.0' 'stateforward:scxml-clock/telemetry@0.2.0' ;;
        # Every data model answers `In()` through execution-host (the null
        # data model's only condition, SCXML B.1).
        scxml_data_model_ecmascript|scxml_data_model_null|scxml_data_model_xpath) printf '%s\n' 'stateforward:scxml/types@0.2.0' 'stateforward:scxml/telemetry@0.2.0' 'stateforward:scxml/execution-host@0.2.0' ;;
        # The example service reaches the session's logical clock only
        # through its world's wasi:clocks/monotonic-clock@0.3.0, and the
        # network only through the host-admitted outbound-http request its
        # `endpoint` slot's reference supplies.
        scxml_service_example) printf '%s\n' 'wasi:clocks/types@0.3.0' 'wasi:clocks/monotonic-clock@0.3.0' 'stateforward:scxml/types@0.2.0' 'stateforward:scxml/outbound-http@0.2.0' 'stateforward:scxml/telemetry@0.2.0' ;;
        # The files example keeps exactly the WASI interfaces of the
        # filesystem it imports: the filesystem interfaces, the wasi:io
        # interfaces they use, and wasi:clocks/wall-clock for its `datetime`
        # type alone (checked by check_wasi_types_only_imports below).
        # wasi:cli and the 0.2 monotonic clock are denied by a composed
        # provider.
        scxml_service_files) printf '%s\n' 'stateforward:scxml/types@0.2.0' 'stateforward:scxml/telemetry@0.2.0' 'wasi:io/poll@0.2.9' 'wasi:clocks/wall-clock@0.2.9' 'wasi:io/error@0.2.9' 'wasi:io/streams@0.2.9' 'wasi:filesystem/types@0.2.9' 'wasi:filesystem/preopens@0.2.9' ;;
        scxml_ioprocessor_scxml) printf '%s\n' 'stateforward:scxml/types@0.2.0' 'stateforward:scxml/telemetry@0.2.0' 'stateforward:scxml-clock/types@0.2.0' 'stateforward:scxml-clock/clock-api@0.2.0' ;;
        # BasicHTTP reaches the network only through the embedder-supplied
        # http-transport capability; wasi:sockets and wasi:http are denied.
        scxml_ioprocessor_basichttp) printf '%s\n' 'stateforward:scxml-basichttp/types@0.2.0' 'stateforward:scxml-basichttp/telemetry@0.2.0' 'stateforward:scxml-basichttp/http-transport@0.2.0' ;;
        # The example namespace exercises every session capability,
        # including outbound-http (with slot references) and the read-only
        # session-snapshot-host.
        scxml_extension_example) printf '%s\n' 'stateforward:scxml/types@0.2.0' 'stateforward:scxml/telemetry@0.2.0' 'stateforward:scxml/data-model-host@0.2.0' 'stateforward:scxml/execution-host@0.2.0' 'stateforward:scxml/outbound-http@0.2.0' 'stateforward:scxml/session-snapshot-host@0.2.0' ;;
        scxml_script_example) printf '%s\n' 'stateforward:scxml/types@0.2.0' 'stateforward:scxml/telemetry@0.2.0' 'stateforward:scxml/data-model-host@0.2.0' ;;
        scxml_reporter_rustc|scxml_reporter_sarif) printf '%s\n' 'stateforward:scxml-reporter/telemetry@0.2.0' 'stateforward:scxml-reporter/types@0.2.0' ;;
        scxml_validator) printf '%s\n' 'stateforward:scxml-validator/telemetry@0.2.0' 'stateforward:scxml-validator/expression-syntax@0.2.0' 'stateforward:scxml-validator/types@0.2.0' ;;
        scxml_compiler) printf '%s\n' 'stateforward:scxml-compiler/telemetry@0.2.0' 'stateforward:scxml-compiler/types@0.2.0' ;;
        scxml_machine) printf '%s\n' 'stateforward:scxml/types@0.2.0' 'stateforward:scxml/telemetry@0.2.0' 'stateforward:scxml/ir-source@0.2.0' 'stateforward:scxml/machine-capability-types@0.2.0' ;;
        # The session snapshot codec is capability-free: it reads only its
        # arguments and reports through the telemetry facade. The machine,
        # data-model, and I/O-processor imports carry only the record types
        # its export uses (no function or resource; the machine snapshot
        # records live in machine-capability-types); types_only_imports
        # checks that below.
        scxml_snapshot_codec) printf '%s\n' 'stateforward:scxml/types@0.2.0' 'stateforward:scxml/telemetry@0.2.0' 'stateforward:scxml/machine-capability-types@0.2.0' 'stateforward:scxml/machine-api@0.2.0' 'stateforward:scxml/data-model-api@0.2.0' 'stateforward:scxml/io-processor-api@0.2.0' ;;
        packaged_machine) printf '%s\n' 'stateforward:scxml/types@0.2.0' 'stateforward:scxml/telemetry@0.2.0' 'stateforward:scxml/machine-capability-types@0.2.0' ;;
        *) echo "no import allowlist for component: $1" >&2; exit 1 ;;
    esac
}

# Interfaces (space-separated, from the `stateforward:scxml@0.2.0` package)
# a component imports for their types alone: the imported
# interface must declare no function and no resource, so the component can
# call nothing through it.
types_only_imports() {
    case "$1" in
        scxml_snapshot_codec) printf '%s\n' 'machine-capability-types machine-api data-model-api io-processor-api' ;;
        *) ;;
    esac
}

# Usage: check_types_only_imports COMPONENT WORLD_TEXT
#
# Reads the `wasm-tools component wit` rendering of COMPONENT and fails when
# an interface named by types_only_imports is missing from the
# `stateforward:scxml@0.2.0` package or declares a function (`name: func`,
# `async func`, `static`, or `constructor`) or a resource.
check_types_only_imports() {
    interfaces=$(types_only_imports "$1")
    test -n "$interfaces" || return 0
    violations=$(printf '%s\n' "$2" | awk -v wanted="$interfaces" '
        BEGIN {
            count = split(wanted, names, " ")
            for (i = 1; i <= count; i++) if (names[i] != "") want[names[i]] = 1
        }
        /^package stateforward:scxml@0\.2\.0 \{$/ { in_package = 1; next }
        /^package / { in_package = 0; next }
        in_package && /^  interface [a-z0-9-]+ \{$/ {
            name = $2
            current = (name in want) ? name : ""
            if (current != "") seen[current] = 1
            next
        }
        in_package && /^  \}$/ { current = ""; next }
        current != "" && (/:[ ]*(async[ ]+)?func[ ]*\(/ || /^[ ]+resource[ ]/ || /^[ ]+constructor[ ]*\(/) {
            print current ": " $0
        }
        END {
            for (name in want) if (!(name in seen)) print name ": interface missing from the component world"
        }
    ')
    test -z "$violations" || {
        echo "types-only import declares a function or resource: $1" >&2
        printf '%s\n' "$violations" >&2
        exit 1
    }
}

# WASI interfaces (space-separated `package/interface` names) a component
# imports for their types alone: the interface must declare no function and
# no resource in the component's world, so the component can call nothing
# through it (the session host links it as an empty instance unless the
# package also declares the permission that grants it).
wasi_types_only_imports() {
    case "$1" in
        scxml_service_files) printf '%s\n' 'wasi:clocks/wall-clock' ;;
        *) ;;
    esac
}

# Usage: check_wasi_types_only_imports COMPONENT WORLD_TEXT
check_wasi_types_only_imports() {
    for interface in $(wasi_types_only_imports "$1"); do
        package=${interface%%/*}
        name=${interface#*/}
        violations=$(printf '%s\n' "$2" | awk -v package="$package" -v wanted="$name" '
            index($0, "package " package "@") == 1 { in_package = 1; next }
            /^package / { in_package = 0; next }
            in_package && $1 == "interface" && $2 == wanted { current = 1; seen = 1; next }
            in_package && /^  \}$/ { current = 0; next }
            current && (/:[ ]*(async[ ]+)?func[ ]*\(/ || /^[ ]+resource[ ]/ || /^[ ]+constructor[ ]*\(/) { print $0 }
            END { if (!seen) print "interface missing from the component world" }
        ')
        test -z "$violations" || {
            echo "WASI types-only import declares a function or resource: $1 $interface" >&2
            printf '%s\n' "$violations" >&2
            exit 1
        }
    done
}

# Example components that import a WASI resource are not host-bundled
# either: an embedder binds them by SHA-256 (authenticate_service_component)
# with the resources it grants, so manifest.tsv does not list them.
caller_authenticated=' scxml_service_files '
in_manifest() {
    case "$caller_authenticated" in
        *" $1 "*) return 1 ;;
        *) return 0 ;;
    esac
}

artifact_count=0
for component in $components; do
    expected_imports "$component" >/dev/null
    if in_manifest "$component"; then
        artifact_count=$((artifact_count + 1))
    fi
done
# One cargo invocation per component: building several packages together
# unifies dependency features across them, so a component's bytes would
# depend on which other components were requested alongside it.
for component in $components; do
    package=$(printf '%s' "$component" | tr '_' '-')
    cargo build --locked --release --target wasm32-wasip2 -p "$package"
done

{
    printf 'schema\tstateforward.wasm-artifacts/v1\n'
    printf 'profile\trelease\n'
    printf 'target\twasm32-wasip2\n'
    printf 'artifact-count\t%s\n' "$artifact_count"
    printf 'wasm-tools\t%s\n' "$expected_wasm_tools"
    printf 'wasi-virt\t%s\n' "$expected_wasi_virt"
} > "$manifest_tmp"

for component in $components; do
    raw="$release_dir/$component.wasm"
    virtualized="$proof_dir/$component.virtualized.wasm"
    repeat="$proof_dir/$component.determinism-check.wasm"
    test -f "$raw" || { echo "missing release component artifact: $raw" >&2; exit 1; }

    virt_input=$raw
    if [ "$component" = scxml_data_model_ecmascript ]; then
        # Boa keys its hash tables through std's RandomState, which reads
        # wasi:random/insecure-seed once; wasi-virt denies randomness, so
        # compose the pinned deterministic seed provider first (see the
        # .wat header). No other WASI capability is satisfied this way.
        seed="$proof_dir/wasi-insecure-seed.wasm"
        seeded="$proof_dir/$component.seeded.wasm"
        wasm-tools parse "$workspace/components/scxml-data-model-ecmascript/wasi-insecure-seed.wat" -o "$seed"
        wasm-tools compose "$raw" -d "$seed" -o "$seeded" 2>/dev/null
        virt_input=$seeded
    fi
    if [ "$component" = scxml_validator ] || [ "$component" = scxml_compiler ]; then
        # The bundled XML Schema backend keys its tables with ahash, which
        # reads wasi:random/random once for its seeds, and std's RandomState
        # (the `event` vocabulary's boon schema checks, for one) reads
        # insecure-seed; the compiler links the same validator. wasi-virt
        # denies randomness, so compose the pinned deterministic provider
        # first (see the .wat header).
        random="$proof_dir/wasi-random-validator.wasm"
        seeded="$proof_dir/$component.seeded.wasm"
        wasm-tools parse "$workspace/components/scxml-validator/wasi-random.wat" -o "$random"
        wasm-tools compose "$raw" -d "$random" -o "$seeded" 2>/dev/null
        virt_input=$seeded
    fi
    if [ "$component" = scxml_service_files ]; then
        # This component imports wasi:filesystem and keeps those imports for
        # the host: a wasi-virt adapter would virtualize the filesystem too
        # (read-only), so the standard library's other WASI imports
        # (wasi:cli and the monotonic clock) are satisfied by the pinned
        # deny-all provider instead (see the .wat header).
        deny="$proof_dir/wasi-cli-deny.wasm"
        wasm-tools parse "$workspace/components/scxml-service-files/wasi-cli-deny.wat" -o "$deny"
        wasm-tools compose "$virt_input" -d "$deny" -o "$virtualized" >/dev/null 2>&1
        wasm-tools compose "$virt_input" -d "$deny" -o "$repeat" >/dev/null 2>&1
    else
        # No other component passes a WASI interface through to its host.
        wasi-virt "$virt_input" -o "$virtualized"
        wasi-virt "$virt_input" -o "$repeat"
    fi
    cmp -s "$virtualized" "$repeat" || { echo "non-deterministic wasi-virt output: $component" >&2; exit 1; }
    wasm-tools validate --features component-model,cm-async "$virtualized"
    world=$(wasm-tools component wit "$virtualized")
    imports=$(printf '%s\n' "$world" | sed -n 's/^  import \(.*\);$/\1/p')
    # A component may keep only a WASI interface its allowlist names (the
    # example service its world's logical clock, and the files example the
    # filesystem it imports).
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
    check_types_only_imports "$component" "$world"
    check_wasi_types_only_imports "$component" "$world"

    size=$(wc -c < "$virtualized" | tr -d ' ')
    digest=$(sha256_file "$virtualized")
    # Paths are canonical relative filenames so identical artifacts do not
    # acquire machine-specific manifest bytes from an absolute build path.
    if in_manifest "$component"; then
        printf 'artifact\t%s\t%s.virtualized.wasm\t%s\t%s\n' "$component" "$component" "$size" "$digest" >> "$manifest_tmp"
    fi
    printf 'verified %s sha256=%s bytes=%s\n' "$component" "$digest" "$size"
done

mv "$manifest_tmp" "$manifest"
manifest_digest=$(sha256_file "$manifest")
printf 'artifact-manifest path=%s sha256=%s\n' "$manifest" "$manifest_digest"

# Codegen-free component-wasm packaging proof. The pinned generic machine
# template is the exact virtualized machine recorded above; the CLI composes
# it with a data-only ir-source provider for a representative machine
# (compound, parallel, deep history, internal raise, ECMAScript). The
# packaged component must be byte-deterministic, validate, import only the
# machine's capability interfaces (never ir-source or WASI), export only
# machine-api, and record the template and IR digests in its recipe. The
# Wasmtime host tests execute these exact bytes and the lowered IR, require
# identical behavior in both IR supply modes, and check it against the pinned
# expected trace.
case " $components " in
    *" scxml_machine "*)
        template="$proof_dir/scxml_machine.virtualized.wasm"
        template_digest=$(sha256_file "$template")
        fixture="$workspace/tests/fixtures/representative-machine.scx"
        ir="$packaged_dir/representative-machine.scxml-ir.json"
        packaged="$packaged_dir/representative-machine.component.wasm"
        report="$packaged_dir/representative-machine.component.report.json"
        mkdir -p "$packaged_dir"
        cargo build --locked -p scxml-cli --no-default-features
        cli="$target_root/debug/scx"
        package() {
            "$cli" compile "$fixture" --policy off --artifact component-wasm \
                --machine-template "$template" --machine-template-sha256 "$template_digest" \
                --output "$1" --canonical "$2" 2>/dev/null
        }
        "$cli" compile "$fixture" --policy off --output "$ir" 2>/dev/null
        "$cli" compile "$fixture" --policy off --output "$packaged_dir/ir.determinism-check.json" 2>/dev/null
        cmp -s "$ir" "$packaged_dir/ir.determinism-check.json" || { echo "non-deterministic lowered IR" >&2; exit 1; }
        package "$packaged" "$report"
        package "$packaged_dir/component.determinism-check.wasm" "$packaged_dir/report.determinism-check.json"
        cmp -s "$packaged" "$packaged_dir/component.determinism-check.wasm" || { echo "non-deterministic packaged component" >&2; exit 1; }
        cmp -s "$report" "$packaged_dir/report.determinism-check.json" || { echo "non-deterministic packaged report" >&2; exit 1; }
        wasm-tools validate --features component-model,cm-async "$packaged"
        world=$(wasm-tools component wit "$packaged")
        imports=$(printf '%s\n' "$world" | sed -n 's/^  import \(.*\);$/\1/p')
        exports=$(printf '%s\n' "$world" | sed -n 's/^  export \(.*\);$/\1/p')
        test "$imports" = "$(expected_imports packaged_machine)" || {
            echo "packaged component import allowlist mismatch" >&2
            printf '%s\n' "$imports" >&2
            exit 1
        }
        test "$exports" = 'stateforward:scxml/machine-api@0.2.0' || {
            echo "packaged component export mismatch" >&2
            printf '%s\n' "$exports" >&2
            exit 1
        }
        ir_digest=$(sha256_file "$ir")
        grep -q "\"template_sha256\": \"$template_digest\"" "$packaged.recipe.json" || { echo "packaged recipe does not pin the template digest" >&2; exit 1; }
        grep -q "\"ir_sha256\": \"$ir_digest\"" "$packaged.recipe.json" || { echo "packaged recipe does not bind the lowered IR digest" >&2; exit 1; }
        packaged_size=$(wc -c < "$packaged" | tr -d ' ')
        packaged_digest=$(sha256_file "$packaged")
        printf 'verified packaged-machine sha256=%s bytes=%s template-sha256=%s ir-sha256=%s\n' "$packaged_digest" "$packaged_size" "$template_digest" "$ir_digest"
        ;;
esac

# Wasm script component proof (SCXML 1.0 section 5.8 `<script src>`). The
# example script component must export only script-api and import only
# types, telemetry, and data-model-host (checked above). The CLI pins it into
# the scripted fixture's lowered IR from a script package (compile-time
# resolution), emits an unpinned runtime-resolution IR, and packages the
# pinned IR as component-wasm with the script component bundled: the package
# must be byte-deterministic, import only the machine's capability
# interfaces, export exactly machine-api and script-bundle, and record the
# script digest in its recipe. The Wasmtime host tests execute these bytes in
# every mode.
has_machine=0
has_script=0
case " $components " in *" scxml_machine "*) has_machine=1 ;; esac
case " $components " in *" scxml_script_example "*) has_script=1 ;; esac
case "$has_machine$has_script" in
    11)
        template="$proof_dir/scxml_machine.virtualized.wasm"
        template_digest=$(sha256_file "$template")
        script="$proof_dir/scxml_script_example.virtualized.wasm"
        script_digest=$(sha256_file "$script")
        fixture="$workspace/tests/fixtures/script-machine.scx"
        scripted_dir="$proof_dir/scripted"
        mkdir -p "$scripted_dir"
        cargo build --locked -p scxml-cli --no-default-features
        cli="$target_root/debug/scx"
        pinned_ir="$scripted_dir/script-machine.scxml-ir.json"
        runtime_ir="$scripted_dir/script-machine.runtime.scxml-ir.json"
        scripted="$scripted_dir/script-machine.component.wasm"
        scripted_report="$scripted_dir/script-machine.component.report.json"
        script_flags() {
            printf '%s\n' --script "example.wasm=$script" --script-sha256 "example.wasm=$script_digest"
        }
        # shellcheck disable=SC2046
        "$cli" compile "$fixture" --policy off --output "$pinned_ir" $(script_flags) 2>/dev/null
        "$cli" compile "$fixture" --policy off --script-resolution runtime --output "$runtime_ir" 2>/dev/null
        scripted_package() {
            # shellcheck disable=SC2046
            "$cli" compile "$fixture" --policy off --artifact component-wasm \
                --machine-template "$template" --machine-template-sha256 "$template_digest" \
                $(script_flags) --output "$1" --canonical "$2" 2>/dev/null
        }
        scripted_package "$scripted" "$scripted_report"
        scripted_package "$scripted_dir/component.determinism-check.wasm" "$scripted_dir/report.determinism-check.json"
        cmp -s "$scripted" "$scripted_dir/component.determinism-check.wasm" || { echo "non-deterministic scripted component" >&2; exit 1; }
        cmp -s "$scripted_report" "$scripted_dir/report.determinism-check.json" || { echo "non-deterministic scripted report" >&2; exit 1; }
        rm -f "$scripted_dir"/*.determinism-check.*
        wasm-tools validate --features component-model,cm-async "$scripted"
        world=$(wasm-tools component wit "$scripted")
        imports=$(printf '%s\n' "$world" | sed -n 's/^  import \(.*\);$/\1/p')
        exports=$(printf '%s\n' "$world" | sed -n 's/^  export \(.*\);$/\1/p')
        test "$imports" = "$(expected_imports packaged_machine)" || {
            echo "scripted component import allowlist mismatch" >&2
            printf '%s\n' "$imports" >&2
            exit 1
        }
        test "$exports" = "$(printf '%s\n' 'stateforward:scxml/machine-api@0.2.0' 'stateforward:scxml/script-bundle@0.2.0')" || {
            echo "scripted component export mismatch" >&2
            printf '%s\n' "$exports" >&2
            exit 1
        }
        grep -q "\"script-sha256:$script_digest\"" "$scripted.recipe.json" || { echo "scripted recipe does not pin the script digest" >&2; exit 1; }
        grep -q "\"sha256\": \"$script_digest\"" "$pinned_ir" || { echo "lowered IR does not pin the script digest" >&2; exit 1; }
        if grep -q "\"pinned\": {" "$runtime_ir"; then echo "runtime-resolution IR must not pin scripts" >&2; exit 1; fi
        scripted_digest=$(sha256_file "$scripted")
        printf 'verified scripted-machine sha256=%s script-sha256=%s\n' "$scripted_digest" "$script_digest"
        ;;
esac
