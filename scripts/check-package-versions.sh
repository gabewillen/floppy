#!/bin/sh
# Usage: check-package-versions.sh TAG
#
# Checks that every package under namespaces/ and services/ is the package of
# release TAG (adapted from scxml.rs/scripts/check-namespace-versions.sh, and
# the counterpart of the reference implementation's scripts/check_release_version.py):
#
#   * namespaces/<name>/package/<name>.extension.json binds the tagless URI
#     https://github.com/stateforward/floppy/namespaces/<name> (a package's
#     identity; a document pins a release with `@<tag>`, which is never part
#     of the URI), its schema's targetNamespace is that exact URI, and its
#     schema-sha256 is the schema's SHA-256;
#   * services/<name>/package/<name>.service.json binds the tagless URI
#     https://github.com/stateforward/floppy/services/<name>, and a
#     schema-artifact it names matches its schema-sha256;
#   * every package-version is TAG without its leading `v`.
#
# A directory that builds a component (it has a Cargo.toml) but has no
# manifest cannot be loaded from a release, so it fails closed.
# scripts/prepare-release.sh runs it first (docs/release.md).
set -eu

test "$#" -eq 1 || { echo "usage: check-package-versions.sh TAG" >&2; exit 2; }
tag=$1
case "$tag" in
    v[0-9]*) version=${tag#v} ;;
    *) echo "TAG must look like vMAJOR.MINOR.PATCH" >&2; exit 2 ;;
esac
workspace=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
. "$workspace/scripts/sha256.sh"
repository='https://github.com/stateforward/floppy'

field() {
    sed -n "s/^  \"$1\": \"\\(.*\\)\",\\{0,1\\}\$/\\1/p" "$2"
}

# check_folder FOLDER SUFFIX URI-FIELD
check_folder() {
    folder=$1
    suffix=$2
    uri_field=$3
    checked=0
    for dir in "$workspace/$folder"/*/; do
        [ -d "$dir" ] || continue
        dir=${dir%/}
        name=$(basename "$dir")
        manifest="$dir/package/$name.$suffix"
        if [ ! -f "$manifest" ]; then
            if [ -f "$dir/Cargo.toml" ]; then
                echo "$folder/$name builds a component but has no package/$name.$suffix" >&2
                exit 1
            fi
            continue
        fi
        expected="$repository/$folder/$name"
        uri=$(field "$uri_field" "$manifest")
        test "$uri" = "$expected" || {
            echo "$folder/$name binds \`$uri\`, not its tagless URI \`$expected\`" >&2
            exit 1
        }
        test "$(field package-version "$manifest")" = "$version" || {
            echo "$folder/$name: package-version is not \`$version\`" >&2
            exit 1
        }
        schema_artifact=$(field schema-artifact "$manifest")
        if [ -n "$schema_artifact" ]; then
            schema="$dir/package/$schema_artifact"
            test -f "$schema" || {
                echo "$folder/$name: its schema-artifact \`$schema_artifact\` is missing" >&2
                exit 1
            }
            test "$(field schema-sha256 "$manifest")" = "$(sha256_file "$schema")" || {
                echo "$folder/$name: schema-sha256 does not match the schema" >&2
                exit 1
            }
            if [ "$folder" = namespaces ]; then
                grep -qF "targetNamespace=\"$expected\"" "$schema" || {
                    echo "$folder/$name: the schema's targetNamespace is not \`$expected\`" >&2
                    exit 1
                }
            fi
        elif [ "$folder" = namespaces ]; then
            echo "$folder/$name: a namespace package names no schema-artifact" >&2
            exit 1
        fi
        checked=$((checked + 1))
    done
    test "$checked" -gt 0 || { echo "no package under $folder/" >&2; exit 1; }
    printf '%s packages are release %s: %s checked\n' "$folder" "$tag" "$checked"
}

check_folder namespaces extension.json namespace-uri
check_folder services service.json service-uri
