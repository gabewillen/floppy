#!/bin/sh
# Regenerates checked-in expected-output fixtures after an intended change.
#
# Usage: sh scripts/bless-fixtures.sh [--ui] [--validator-rules] [--trace-fixture] [--session-snapshot] [--all]
#
#   --ui               tests/ui snapshots (scx `ui` test)
#   --validator-rules  the generated diagnostic-code table in
#                      docs/validator-rules.md (scxml-validator-core)
#   --trace-fixture    the shared Wasmtime/browser trace fixture
#                      (scxml-host-wasmtime `machine_ir_modes` test)
#   --session-snapshot the deterministic session-snapshot goldens and
#                      consecutive-checkpoint diff goldens under
#                      tests/fixtures/session-snapshot/{golden,diff}
#                      (scxml-host-wasmtime `session_snapshot` test)
#   --all              every fixture above
#
# The flags are the supported interface. Each one runs exactly one locked
# cargo test invocation with that test's bless switch set for that process
# only: SCXML_BLESS=1 for --ui, --validator-rules, and --session-snapshot,
# SCXML_UPDATE_TRACE_FIXTURE=1 for --trace-fixture. The variables are the
# test harness's internal mechanism (libtest has no custom flags); ordinary
# test runs never set them, so they never rewrite a fixture. Review the
# resulting diff before committing.
set -eu

usage() {
    echo 'usage: sh scripts/bless-fixtures.sh [--ui] [--validator-rules] [--trace-fixture] [--session-snapshot] [--all]' >&2
    exit 2
}

ui=0
validator_rules=0
trace_fixture=0
session_snapshot=0
[ "$#" -gt 0 ] || usage
for argument in "$@"; do
    case "$argument" in
        --ui) ui=1 ;;
        --validator-rules) validator_rules=1 ;;
        --trace-fixture) trace_fixture=1 ;;
        --session-snapshot) session_snapshot=1 ;;
        --all) ui=1; validator_rules=1; trace_fixture=1; session_snapshot=1 ;;
        *) usage ;;
    esac
done

workspace=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
cd "$workspace"

if [ "$ui" -eq 1 ]; then
    SCXML_BLESS=1 cargo test --locked -p scxml-cli --test ui
fi
if [ "$validator_rules" -eq 1 ]; then
    SCXML_BLESS=1 cargo test --locked -p scxml-validator-core --lib \
        validator_rules_document_embeds_the_generated_code_table
fi
if [ "$trace_fixture" -eq 1 ]; then
    SCXML_UPDATE_TRACE_FIXTURE=1 cargo test --locked -p scxml-host-wasmtime \
        --features wasmtime --test machine_ir_modes
fi
if [ "$session_snapshot" -eq 1 ]; then
    SCXML_BLESS=1 cargo test --locked -p scxml-host-wasmtime \
        --features wasmtime --test session_snapshot
fi
