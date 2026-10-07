#!/bin/sh
# Builds and proves the browser host (hosts/scxml-host-browser).
#
# Usage: sh scripts/verify-browser-host.sh [--skip-e2e]
#
#   --skip-e2e   skip the headless Chromium run (reported as SKIPPED)
#
#  1. builds, virtualizes, and authenticates the release machine, compiler,
#     ECMAScript, XPath, and null data-model, SCXML and BasicHTTP I/O
#     processor, clock, and optional session snapshot codec guests
#     (scripts/verify-wasm-capabilities.sh, reproducible flags), and compiles
#     the representative fixture to lowered IR with the CLI;
#  2. installs the pinned npm dependencies with `npm ci` from the committed
#     lockfile (the only network access; lifecycle scripts disabled) and
#     typechecks the package's public declarations (`src/index.d.ts`,
#     strict, with the pinned TypeScript);
#  3. builds the in-workspace scxml-wasm-meter tool (cargo --locked), then
#     transpiles each component once with the pinned jco transpiler and meters
#     every core module, twice into separate directories, and requires
#     byte-identical output;
#  4. runs the Node smoke, contract, capability, and IRP parity tests (the
#     same ES modules and metered core Wasm a browser loads): the shared
#     trace fixture, hostile IR, deterministic fuel exhaustion of hostile
#     loops, argument validation, drift of index.d.ts and the closed
#     vocabularies against WIT and Rust, every machine capability, the
#     page-allowlisted URL resources and their bounds, the BasicHTTP
#     processor (deny-by-default, allowlist, encoding, error.communication,
#     inbound decoding), and the official W3C IRP cases (static `src`
#     documents through resources, BasicHTTP over the in-process loopback)
#     against the checked-in Wasmtime
#     baselines, plus the session snapshot API and golden parity, and the
#     async bridge (`async: 'auto'`: its worker mechanism in a real thread,
#     prefetched resources sent to the worker, and its typed
#     `dynamic-url-unavailable` fallback);
#  5. runs the same trace, a hostile nested loop, an event-loop delayed
#     send, an invocation, an allowlisted `<data src>` through resources, and
#     BasicHTTP over the default fetch (allowlisted, denied, and inbound) in
#     headless Chromium through the pinned playwright-core, plus the async
#     bridge page cross-origin isolated (`worker`) and not (`none`), unless
#     --skip-e2e.
#
# Every node:test run must report `todo 0`, `skipped 0`, `cancelled 0`, and
# `fail 0`: no test may pass as todo or skipped.
#
# Pinned tools: Node.js v24.11.1 (and its bundled npm), jco-transpile 0.14.0 and
# playwright-core 1.62.0 via package-lock.json, scxml-wasm-meter from this
# workspace, plus the wasm-tools and wasi-virt versions
# verify-wasm-capabilities.sh pins.
set -eu

skip_e2e=0
for argument in "$@"; do
    case "$argument" in
        --skip-e2e) skip_e2e=1 ;;
        *)
            echo "usage: sh scripts/verify-browser-host.sh [--skip-e2e]" >&2
            exit 2
            ;;
    esac
done

expected_node='v24.11.1'
test "$(node --version)" = "$expected_node" || {
    echo "node identity mismatch: expected $expected_node, found $(node --version)" >&2
    exit 1
}

workspace=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
. "$workspace/scripts/sha256.sh"
package="$workspace/hosts/scxml-host-browser"
target_root=${CARGO_TARGET_DIR:-target}
case "$target_root" in
    /*) ;;
    *) target_root="$workspace/$target_root" ;;
esac
export CARGO_TARGET_DIR="$target_root"
# Its own artifact directory: the default release directory is the one `scx`
# embeds (crates/scxml-cli/build.rs), which a nine-guest subset must not
# replace.
artifacts="$target_root/wasm-artifacts/browser-host"
ir="$artifacts/packaged/representative-machine.scxml-ir.json"
check_dist="$target_root/browser-host/dist.determinism-check"
tap="$target_root/browser-host/node-test.tap"
trap 'rm -rf "$check_dist" "$tap"' EXIT HUP INT TERM

# Runs node:test with the spec reporter on stdout and TAP into $tap, then
# requires a summary with no todo, skipped, cancelled, or failed test.
run_node_tests() {
    mkdir -p "$(dirname -- "$tap")"
    node --test --test-reporter=spec --test-reporter-destination=stdout \
        --test-reporter=tap --test-reporter-destination="$tap" "$@" || return 1
    summary=$(tail -n 12 "$tap")
    for counter in todo skipped cancelled fail; do
        printf '%s\n' "$summary" | grep -qx "# $counter 0" || {
            echo "node:test summary lacks '# $counter 0':" >&2
            printf '%s\n' "$summary" >&2
            return 1
        }
    done
    printf '%s\n' "$summary" | grep '^# \(tests\|pass\|fail\|todo\|skipped\|cancelled\) '
}

sh "$workspace/scripts/verify-wasm-capabilities.sh" --artifacts-dir "$artifacts" \
    scxml_clock scxml_data_model_ecmascript scxml_data_model_null scxml_data_model_xpath \
    scxml_ioprocessor_scxml scxml_ioprocessor_basichttp scxml_compiler scxml_machine \
    scxml_snapshot_codec
test -f "$ir" || { echo "missing CLI-compiled lowered IR: $ir" >&2; exit 1; }
cargo build --release --locked --manifest-path "$workspace/Cargo.toml" -p scxml-wasm-meter
meter="$target_root/release/scxml-wasm-meter"

cd "$package"
npm ci --ignore-scripts --no-audit --no-fund
npm run typecheck
node scripts/build.mjs --artifacts "$artifacts" --meter "$meter"
node scripts/build.mjs --artifacts "$artifacts" --meter "$meter" --out "$check_dist"
diff -r dist "$check_dist" >/dev/null || { echo "non-deterministic jco transpile output" >&2; exit 1; }
printf 'browser-host dist build-manifest sha256=%s\n' "$(sha256_file dist/build-manifest.json)"

SCXML_BROWSER_IR="$ir" run_node_tests test/smoke.test.mjs test/contract.test.mjs \
    test/capabilities.test.mjs test/resources.test.mjs test/basichttp.test.mjs \
    test/irp-parity.test.mjs test/snapshot.test.mjs test/snapshot-parity.test.mjs \
    test/async-bridge.test.mjs || exit 1

if [ "$skip_e2e" = 1 ]; then
    echo 'SKIPPED: headless Chromium e2e (--skip-e2e)'
    exit 0
fi
SCXML_BROWSER_IR="$ir" run_node_tests test/browser.test.mjs || {
    echo "headless Chromium e2e failed; if the pinned headless shell is missing, install it with:" >&2
    echo "  (cd hosts/scxml-host-browser && npx playwright-core install --only-shell chromium)" >&2
    echo "or skip it explicitly with --skip-e2e" >&2
    exit 1
}
