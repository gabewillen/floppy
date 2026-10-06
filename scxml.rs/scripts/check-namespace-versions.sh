#!/bin/sh
# Usage: check-namespace-versions.sh TAG
#
# Checks that every repository namespace under namespaces/ names exactly TAG:
# namespaces/<name>/package/<name>.extension.json binds the namespace URI
# https://github.com/stateforward/scxml.rs/namespaces/<name>@TAG, its schema's
# targetNamespace is that exact URI, and its schema-sha256 is the schema's
# SHA-256. The release job runs it before publishing TAG, so a release never
# serves a package whose namespace names another version
# (docs/executable-content-extensions.md, "Loading an element from a
# repository"). scripts/set-namespace-version.sh moves every repository
# namespace to a new tag.
set -eu

test "$#" -eq 1 || { echo "usage: check-namespace-versions.sh TAG" >&2; exit 2; }
tag=$1
workspace=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
. "$workspace/scripts/sha256.sh"
repository='https://github.com/stateforward/scxml.rs'

field() {
    sed -n "s/^  \"$1\": \"\\(.*\\)\",\\{0,1\\}\$/\\1/p" "$2"
}

checked=0
for dir in "$workspace"/namespaces/*/; do
    dir=${dir%/}
    name=$(basename "$dir")
    manifest="$dir/package/$name.extension.json"
    # A namespace directory is a repository namespace exactly when it ships
    # the package manifest package/<name>.extension.json; only those carry a
    # release tag in their URI. A directory without one is an unversioned
    # vocabulary (namespaces/env and namespaces/event, the host's attribute
    # vocabularies) and is skipped,
    # but a directory that builds an extension component (it has a
    # Cargo.toml) cannot be loaded without its manifest, so it fails closed.
    if [ ! -f "$manifest" ]; then
        if [ -f "$dir/Cargo.toml" ]; then
            echo "namespace $name builds a component but has no package/$name.extension.json" >&2
            exit 1
        fi
        continue
    fi
    expected="$repository/namespaces/$name@$tag"
    uri=$(field namespace-uri "$manifest")
    test "$uri" = "$expected" || {
        echo "namespace $name binds \`$uri\`, not \`$expected\`" >&2
        exit 1
    }
    schema="$dir/package/$(field schema-artifact "$manifest")"
    grep -qF "targetNamespace=\"$expected\"" "$schema" || {
        echo "namespace $name: the schema's targetNamespace is not \`$expected\`" >&2
        exit 1
    }
    test "$(field schema-sha256 "$manifest")" = "$(sha256_file "$schema")" || {
        echo "namespace $name: schema-sha256 does not match the schema" >&2
        exit 1
    }
    checked=$((checked + 1))
done
test "$checked" -gt 0 || { echo "no repository namespace under namespaces/" >&2; exit 1; }
printf 'repository namespaces name %s: %s checked\n' "$tag" "$checked"
