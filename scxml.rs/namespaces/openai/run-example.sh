#!/bin/sh
# Runs one example (examples/hello.scxml by default) through `scxml-cli
# snapshot run` on the Wasmtime host with the locally built openai package
# bound, then renders the final session snapshot.
#
# Usage: sh namespaces/openai/run-example.sh RELEASE_DIR BASE_URL [MODEL [EXAMPLE]]
#
#   RELEASE_DIR  artifacts of scripts/verify-wasm-capabilities.sh (with
#                scxml_extension_openai and the session-host components)
#   BASE_URL     the OPENAI_BASE_URL value the element's env:baseurl slot
#                names, e.g. https://api.openai.com/v1, or
#                http://127.0.0.1:11434/v1 for Ollama (a base URL the run
#                supplies is an origin the element may reach; the package
#                itself declares https://api.openai.com)
#   MODEL        the model (default: the document's gpt-4o-mini)
#   EXAMPLE      hello (content generation, the default) or decide
#                (function calling)
#
# OPENAI_API_KEY must be set in the invoking environment (any non-empty
# value for servers that ignore it); `--env OPENAI_API_KEY` makes the run hold
# it, and the element's env:apikey slot names it. The extension component
# receives only an opaque reference, never the key. Requires
# `cargo build -p scxml-cli --features wasmtime`.
set -eu
if [ "$#" -lt 2 ]; then
    sed -n '6,22p' "$0" >&2
    exit 2
fi
release=$1
base=$2
model=${3:-gpt-4o-mini}
example=${4:-hello}
case "$example" in
    hello|decide) ;;
    *) echo "unknown example: $example (hello or decide)" >&2; exit 2 ;;
esac
workspace=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd -P)
. "$workspace/scripts/sha256.sh"
cli="${CARGO_TARGET_DIR:-$workspace/target}/debug/scxml-cli"
uri='https://github.com/stateforward/scxml.rs/namespaces/openai@v0.2.0'
manifest_sha=$(sha256_file "$release/manifest.tsv")
# A repository namespace is not listed in manifest.tsv; this local example pins
# the bytes the capability script just built and verified.
openai_sha=$(sha256_file "$release/scxml_extension_openai.virtualized.wasm")
work=$(mktemp -d "${TMPDIR:-/tmp}/openai-example.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM
# The document names its model through `modelexpr="model"`; the copy sets
# that variable's initial value.
sed "s|expr=\"'gpt-4o-mini'\"|expr=\"'$model'\"|" \
    "$workspace/namespaces/openai/examples/$example.scxml" > "$work/$example.scxml"
OPENAI_BASE_URL=$base "$cli" snapshot run "$work/$example.scxml" \
    --components "$release" --components-manifest-sha256 "$manifest_sha" \
    --extension-package "$workspace/namespaces/openai/package/openai.extension.json" \
    --extension-component "$uri=$release/scxml_extension_openai.virtualized.wasm" \
    --extension-component-sha256 "$uri=$openai_sha" \
    --env OPENAI_API_KEY --env OPENAI_BASE_URL \
    --output "$work/final.snapshot.json"
"$cli" snapshot show "$work/final.snapshot.json" --color never
