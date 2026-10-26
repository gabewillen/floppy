#!/bin/sh
# Builds the workshop's runtime into target/web/host: scxml.rs's browser host
# (scxml.rs/hosts/scxml-host-browser) with floppy's namespace components
# shipped beside its own, so the workshop validates, compiles and runs a bot
# in the page with the same Wasm components `scx` runs.
#
# Usage: sh web/build-host.sh
#
# 1. The components the browser host ships (machine, compiler, validator,
#    data models, I/O processors, clock, snapshot codec, the example and
#    `scx` namespaces), built by the subtree's own
#    scripts/verify-wasm-capabilities.sh into target/web/scxml.
# 2. floppy's components (scripts/build-components.sh) into
#    target/web/components; its namespace components are listed in
#    target/web/extensions/manifest.tsv for the host build's --extensions.
# 3. The browser host package, copied to target/web/host-package (the
#    subtree is never written), its pinned npm dependencies installed with
#    `npm ci`, and built with scripts/build.mjs into target/web/host/dist; its
#    src/ and the OpenTelemetry API's ES modules are copied beside it.
#
# Needs what scxml.rs's browser host build needs (Node.js v24.11.1, the
# pinned wasm-tools and wasi-virt) and what build-components.sh needs
# (wasi-sdk 25 at WASI_SDK_PATH).
set -eu

workspace=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
. "$workspace/scripts/sha256.sh"
out="$workspace/target/web"
scxml_target="$workspace/target/scxml.rs"
browser_components='scxml_clock scxml_data_model_ecmascript scxml_data_model_null scxml_data_model_xpath
    scxml_ioprocessor_scxml scxml_ioprocessor_basichttp scxml_compiler scxml_machine
    scxml_snapshot_codec scxml_validator scxml_extension_example scxml_package_validator_example scx'

expected_node='v24.11.1'
test "$(node --version)" = "$expected_node" || {
    echo "node identity mismatch: expected $expected_node, found $(node --version)" >&2
    exit 1
}

# 1. scxml.rs's components.
(
    cd "$workspace/scxml.rs"
    unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_TARGET_WASM32_WASIP2_RUSTFLAGS
    # shellcheck disable=SC2086
    CARGO_TARGET_DIR="$scxml_target" \
        sh scripts/verify-wasm-capabilities.sh --artifacts-dir "$out/scxml" $browser_components
    CARGO_TARGET_DIR="$scxml_target" cargo build --release --locked -p scxml-wasm-meter
)

# 2. floppy's components, and the namespace components as extensions.
sh "$workspace/scripts/build-components.sh" --artifacts-dir "$out/components"
rm -rf "$out/extensions"
mkdir -p "$out/extensions"
{
    printf 'schema\tstateforward.wasm-artifacts/v1\n'
    grep '^artifact	' "$out/components/namespaces/components.tsv"
} > "$out/extensions/manifest.tsv"
grep '^artifact	' "$out/extensions/manifest.tsv" | cut -f 3 | while read -r file; do
    cp "$out/components/namespaces/$file" "$out/extensions/$file"
done

# 3. The browser host.
package="$out/host-package"
rm -rf "$package" "$out/host"
mkdir -p "$package" "$out/host"
cp -R "$workspace/scxml.rs/hosts/scxml-host-browser/." "$package/"
rm -rf "$package/node_modules" "$package/dist"
(
    cd "$package"
    npm ci --ignore-scripts --no-audit --no-fund
    node scripts/build.mjs --artifacts "$out/scxml" --meter "$scxml_target/release/scxml-wasm-meter" \
        --extensions "$out/extensions" --out "$out/host/dist"
)
cp -R "$package/src" "$out/host/src"
cp "$workspace/scxml.rs/LICENSE-APACHE" "$workspace/scxml.rs/LICENSE-MIT" "$out/host/"
cp "$workspace/scxml.rs/crates/scxml-validator-xsd/schemas/w3c-scxml-1.0/NOTICE.md" "$out/host/NOTICE.md"
mkdir -p "$out/host/opentelemetry-api"
cp -R "$package/node_modules/@opentelemetry/api/build/esm/." "$out/host/opentelemetry-api/"
cp "$package/node_modules/@opentelemetry/api/LICENSE" "$out/host/opentelemetry-api/LICENSE"
# Its ES modules import each other without the .js a browser needs (bundlers
# and Node.js add it): add it to every relative specifier, each of which must
# name a file or a directory's index.js there.
node -e '
const fs = require("fs"), path = require("path");
const walk = d => fs.readdirSync(d, { withFileTypes: true }).flatMap(e => e.isDirectory() ? walk(path.join(d, e.name)) : [path.join(d, e.name)]);
for (const file of walk(process.argv[1]).filter(f => f.endsWith(".js"))) {
  const text = fs.readFileSync(file, "utf8").replace(/((?:from|import)\s*\(?\s*["\x27])(\.{1,2}\/[^"\x27]+?)(["\x27])/g, (all, head, spec, tail) => {
    if (spec.endsWith(".js")) return all;
    const at = path.join(path.dirname(file), spec);
    if (fs.existsSync(at + ".js")) return head + spec + ".js" + tail;
    if (fs.existsSync(path.join(at, "index.js"))) return head + spec.replace(/\/?$/, "/") + "index.js" + tail;
    throw new Error(`${file}: ${spec} names no file`);
  });
  fs.writeFileSync(file, text);
}' "$out/host/opentelemetry-api"
printf 'web-host dir=%s build-manifest-sha256=%s\n' "$out/host" "$(sha256_file "$out/host/dist/build-manifest.json")"
