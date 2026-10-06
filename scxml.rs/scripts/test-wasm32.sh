#!/bin/sh
# Runs every test named `wasm_only_*` on wasm32-wasip2 under Wasmtime.
#
# Resource bounds apply to wasm builds only (`scxml_types::wasm_bound`), so a
# test that checks one of them is compiled for wasm32 only and never runs in
# a native `cargo test`. The test binaries below import nothing but WASI, so
# the Wasmtime CLI runs them; a crate whose wasm32 build imports a project
# WIT interface (a component crate, or one that records telemetry through the
# host) pins its documented bound values natively and has the host's
# component tests check each bound through the shipped component instead.
set -eu
cd "$(dirname "$0")/.."
export CARGO_TARGET_WASM32_WASIP2_RUNNER="${CARGO_TARGET_WASM32_WASIP2_RUNNER:-wasmtime run --dir=.}"

# The guards below read the tracked sources: outside a git work tree they
# cannot, so fail closed rather than find nothing.
if [ "$(git rev-parse --is-inside-work-tree 2>/dev/null || true)" != true ]; then
  echo "test-wasm32.sh: not inside a git work tree; its source guards cannot run" >&2
  exit 1
fi

# The tracked Rust files containing the pattern $1, one per line. `git grep`
# exits 1 when nothing matches; any other failure fails the caller.
rust_files_with() {
  status=0
  files=$(git grep -l "$1" -- '*.rs') || status=$?
  case "$status" in
    0) printf '%s\n' "$files" ;;
    1) ;;
    *)
      echo "test-wasm32.sh: git grep failed (status $status)" >&2
      return 2
      ;;
  esac
}

# Self-check of the helper: a pattern no tracked file contains yields no
# files and succeeds; a pattern git grep cannot compile fails.
if ! none=$(rust_files_with 'test-wasm32 self-check: no file contains this') || [ -n "$none" ]; then
  echo "test-wasm32.sh: self-check failed: a pattern with no match must yield no files" >&2
  exit 1
fi
if rust_files_with '[' >/dev/null 2>&1; then
  echo "test-wasm32.sh: self-check failed: a git grep error must fail the helper" >&2
  exit 1
fi

# Fail closed when a `wasm_only_` test lives outside the targets run below.
known='^(crates/scxml-compiler-core/src/|crates/scxml-compiler-core/tests/ir_contract\.rs|crates/scxml-engine/src/|crates/scxml-engine/tests/appendix_d\.rs|crates/scxml-packager/src/|crates/scxml-packager/tests/package_contract\.rs|crates/scxml-script/src/|crates/scxml-types/src/|crates/scxml-validator-xsd/src/|crates/scxml-xpath/tests/(language|tree)\.rs)'
wasm_only=$(rust_files_with 'fn wasm_only_') || exit 1
stray=$(printf '%s\n' "$wasm_only" | grep -Ev "$known" | grep -v '^$' || true)
if [ -n "$stray" ]; then
  echo "wasm_only_ tests outside the wasm32 test targets:" >&2
  echo "$stray" >&2
  exit 1
fi

# Fail closed when test code gates anything but a `wasm_only_` test on
# wasm32: a `#[cfg(target_arch = "wasm32")]` block or a
# `cfg!(target_arch = "wasm32")` branch in a test never runs here. Test code
# is every file under a `tests/` directory, every `tests.rs` or `*_tests.rs`
# module file, and every `#[cfg(test)]` module.
wasm_gated=$(rust_files_with 'target_arch = "wasm32"') || exit 1
gated=$(printf '%s\n' "$wasm_gated" | while read -r file; do
  [ -n "$file" ] || continue
  awk -v file="$file" '
    BEGIN { test = (file ~ /(^|\/)tests\// || file ~ /(^|\/|_)tests\.rs$/) }
    gate { gate = 0; if ($0 ~ /^[ \t]*(pub(\([a-z]+\))? )?mod /) test = 1 }
    /^[ \t]*#\[cfg\(test\)\]/ { gate = 1 }
    pending {
      if ($0 ~ /^[ \t]*(#\[|\/\/\/)/) next
      if ($0 !~ /fn wasm_only_/) print file ":" line
      pending = 0
    }
    test && /#\[cfg\(target_arch = "wasm32"\)\]/ { pending = 1; line = NR }
    test && /[^!]cfg!\(target_arch = "wasm32"\)/ { print file ":" NR }
  ' "$file"
done)
if [ -n "$gated" ]; then
  echo "test code gated on wasm32 outside a wasm_only_ test:" >&2
  echo "$gated" >&2
  exit 1
fi

cargo test --locked --target wasm32-wasip2 -p scxml-compiler-core -p scxml-engine -p scxml-packager -p scxml-script -p scxml-types -p scxml-validator-xsd --lib -- wasm_only_
cargo test --locked --target wasm32-wasip2 -p scxml-compiler-core --test ir_contract -- wasm_only_
cargo test --locked --target wasm32-wasip2 -p scxml-engine --test appendix_d -- wasm_only_
cargo test --locked --target wasm32-wasip2 -p scxml-packager --test package_contract -- wasm_only_
cargo test --locked --target wasm32-wasip2 -p scxml-xpath --test language --test tree -- wasm_only_
