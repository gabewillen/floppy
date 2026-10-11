#!/bin/sh
# Prepares and measures floppy's namespace implementation report plans
# (namespaces/<name>/irp), the way scxml.rs prepares and measures its own
# namespace suites (scxml.rs/docs/conformance.md, "Namespace suites").
#
# Usage: sh scripts/namespace-irp.sh prepare|baselines NAME...
#
# prepare    rewrites namespaces/NAME/irp/prepared/ecmascript from the
#            checked-in sources and manifest with `scx conformance prepare
#            --namespace NAME`: the unmodified W3C templates
#            (scxml.rs/tests/conformance/w3c-irp/templates) applied by
#            SaxonJ-HE 12.8, named by $SAXON (default `saxon`; for example
#            `java -jar /opt/saxon/saxon-he-12.8.jar`). On macOS it prepares
#            offline under sandbox-exec, as scxml.rs does; scx has no offline
#            isolation on other hosts, so there it prepares without
#            --offline (the corpus records which). floppy's documents are
#            ECMAScript only, so only the ecmascript corpus is prepared.
# baselines  runs each prepared corpus through `scx conformance run` with
#            NAME's extension package bound and writes its report (the
#            `--json` report) to namespaces/NAME/irp/baselines/
#            ecmascript.report.json and its SHA256SUMS. It fails unless every
#            case passes.
#            Regenerate the baselines on Linux x86_64 (the host
#            tests/tests/namespace_irp.rs compares fuel on) after any change
#            to a namespace, its suite, or the scxml.rs subtree.
#
# Both use target/scx/debug/scx and the components in
# target/test-components, which `cargo test -p floppy-tests` builds (any
# test that runs scx). baselines assembles the session components and the
# namespace's component into one release directory, target/irp-release,
# since `scx conformance run` reads an extension-artifact from --components.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
. "$root/scripts/sha256.sh"
scx="$root/target/scx/debug/scx"
components="$root/target/test-components"
# Paths the corpus and report record are relative to the workspace root, so
# they read the same on every host.
cd "$root"

usage() {
    echo "usage: namespace-irp.sh prepare|baselines NAME..." >&2
    exit 2
}

[ $# -ge 2 ] || usage
action=$1
shift
[ -x "$scx" ] || { echo "namespace-irp.sh: build $scx first (cargo test -p floppy-tests --test smoke)" >&2; exit 1; }

case "$action" in
    prepare)
        offline=
        [ "$(uname -s)" = Darwin ] && offline=--offline
        for name in "$@"; do
            suite="namespaces/$name/irp"
            [ -f "$suite/manifest.xml" ] || { echo "namespace-irp.sh: no suite $suite" >&2; exit 1; }
            cache=$(mktemp -d)
            rm -rf "$suite/prepared/ecmascript"
            # JAVA_TOOL_OPTIONS makes the JVM print a banner the transformer
            # identity would read.
            env -u JAVA_TOOL_OPTIONS "$scx" conformance prepare \
                --manifest "$suite/manifest.xml" --source-dir "$suite/sources" \
                --templates scxml.rs/tests/conformance/w3c-irp/templates \
                --datamodel ecmascript --cache "$cache" \
                --output "$suite/prepared/ecmascript" \
                --revision 0.1.0 --namespace "$name" --saxon "${SAXON:-saxon}" $offline
            rm -rf "$cache"
        done
        ;;
    baselines)
        release="$root/target/irp-release"
        rm -rf "$release"
        cp -R "$components/scxml" "$release"
        manifest_sha256=$(sha256_file "$release/manifest.tsv")
        for name in "$@"; do
            suite="namespaces/$name/irp"
            cp "$components/namespaces/floppy_$name.virtualized.wasm" "$release/"
            component_sha256=$(sha256_file "$release/floppy_$name.virtualized.wasm")
            mkdir -p "$suite/baselines"
            "$scx" conformance run --corpus "$suite/prepared/ecmascript" \
                --components "$release" --components-manifest-sha256 "$manifest_sha256" \
                --extension-package "namespaces/$name/package/$name.extension.json" \
                --extension-component-sha256 "https://github.com/stateforward/floppy/namespaces/$name=$component_sha256" \
                --json "$suite/baselines/ecmascript.report.json"
            digest=$(sha256_file "$suite/baselines/ecmascript.report.json")
            printf '%s  ecmascript.report.json\n' "$digest" > "$suite/baselines/SHA256SUMS"
        done
        ;;
    *)
        usage
        ;;
esac
