#!/bin/sh
# Usage: check-namespace-versions.sh TAG
#
# Checks that every namespace package under namespaces/ is the package of
# release TAG: namespaces/<name>/package/<name>.extension.json binds the
# tagless namespace URI https://github.com/stateforward/scxml.rs/namespaces/<name>
# (a package's identity; a document pins a release with `@<tag>`, which is
# never part of the namespace), its schema's targetNamespace is that exact
# URI, its schema-sha256 is the schema's SHA-256, and its package-version is
# TAG without its leading `v`; and that every data-model package under
# datamodels/ is too: datamodels/<name>/package/<name>.data-model.json binds
# https://github.com/stateforward/scxml.rs/datamodels/<name> and has TAG's
# package-version.
# The release job runs it before publishing TAG
# (docs/executable-content-extensions.md, "Loading an element from a
# repository").
set -eu

test "$#" -eq 1 || { echo "usage: check-namespace-versions.sh TAG" >&2; exit 2; }
tag=$1
case "$tag" in
    v[0-9]*) version=${tag#v} ;;
    *) echo "TAG must look like vMAJOR.MINOR.PATCH" >&2; exit 2 ;;
esac
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
    # A namespace directory ships a package exactly when it has the manifest
    # package/<name>.extension.json. A directory without one is a vocabulary
    # the host interprets (namespaces/env, namespaces/event, and
    # namespaces/datamodel) and is
    # skipped, but a directory that builds an extension component (it has a
    # Cargo.toml) cannot be loaded without its manifest, so it fails closed.
    if [ ! -f "$manifest" ]; then
        if [ -f "$dir/Cargo.toml" ]; then
            echo "namespace $name builds a component but has no package/$name.extension.json" >&2
            exit 1
        fi
        continue
    fi
    expected="$repository/namespaces/$name"
    uri=$(field namespace-uri "$manifest")
    test "$uri" = "$expected" || {
        echo "namespace $name binds \`$uri\`, not its tagless URI \`$expected\`" >&2
        exit 1
    }
    test "$(field package-version "$manifest")" = "$version" || {
        echo "namespace $name: package-version is not \`$version\`" >&2
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
test "$checked" -gt 0 || { echo "no namespace package under namespaces/" >&2; exit 1; }
printf 'namespace packages are release %s: %s checked\n' "$tag" "$checked"

models=0
for dir in "$workspace"/datamodels/*/; do
    [ -d "$dir" ] || continue
    dir=${dir%/}
    name=$(basename "$dir")
    manifest="$dir/package/$name.data-model.json"
    test -f "$manifest" || { echo "data model $name has no package/$name.data-model.json" >&2; exit 1; }
    expected="$repository/datamodels/$name"
    test "$(field data-model-uri "$manifest")" = "$expected" || {
        echo "data model $name does not bind its tagless URI \`$expected\`" >&2
        exit 1
    }
    test "$(field package-version "$manifest")" = "$version" || {
        echo "data model $name: package-version is not \`$version\`" >&2
        exit 1
    }
    models=$((models + 1))
done
printf 'data-model packages are release %s: %s checked\n' "$tag" "$models"
