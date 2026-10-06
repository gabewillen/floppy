#!/bin/sh
# Usage: set-namespace-version.sh TAG
#
# Moves every repository namespace under namespaces/ to the namespace URI of the
# release TAG: https://github.com/stateforward/scxml.rs/namespaces/<name>@TAG.
# A repository namespace's URI carries its version, so a new release
# is a new namespace: this rewrites the namespace's current URI byte for byte
# in every tracked file that names it (manifest, schema targetNamespace,
# component constant, examples, tests, and docs), then recomputes the
# manifest's schema-sha256. Run it, review the diff, and commit it before
# tagging; the release job checks the result with check-namespace-versions.sh.
set -eu

test "$#" -eq 1 || { echo "usage: set-namespace-version.sh TAG" >&2; exit 2; }
tag=$1
case "$tag" in
    v[0-9]*) ;;
    *) echo "TAG must look like vMAJOR.MINOR.PATCH" >&2; exit 2 ;;
esac
case "$tag" in
    *[!A-Za-z0-9.+-]*) echo "TAG may contain only letters, digits, '.', '+', and '-'" >&2; exit 2 ;;
esac
workspace=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
. "$workspace/scripts/sha256.sh"
repository='https://github.com/stateforward/scxml.rs'
cd "$workspace"

field() {
    sed -n "s/^  \"$1\": \"\\(.*\\)\",\\{0,1\\}\$/\\1/p" "$2"
}

for dir in namespaces/*/; do
    name=$(basename "$dir")
    manifest="${dir}package/$name.extension.json"
    # A namespace directory is a repository namespace exactly when it ships
    # the package manifest package/<name>.extension.json; only those carry a
    # release tag in their URI. A directory without one is an unversioned
    # vocabulary (namespaces/env and namespaces/event, the host's attribute
    # vocabularies) and is skipped,
    # but a directory that builds an extension component (it has a
    # Cargo.toml) cannot be loaded without its manifest, so it fails closed.
    if [ ! -f "$manifest" ]; then
        if [ -f "${dir}Cargo.toml" ]; then
            echo "namespace $name builds a component but has no package/$name.extension.json" >&2
            exit 1
        fi
        continue
    fi
    # namespaces/scx ships inside scx (it embeds the package and component),
    # so its URI is the tagless https://github.com/stateforward/scxml.rs/namespaces/scx,
    # versioned with the toolchain like the env and event vocabularies; it
    # carries no release tag and is skipped.
    if [ "$(field namespace-uri "$manifest")" = "$repository/namespaces/$name" ]; then
        continue
    fi
    old=$(field namespace-uri "$manifest")
    new="$repository/namespaces/$name@$tag"
    if [ "$old" != "$new" ]; then
        # The URI contains '/', '.', and '@' but never '|'. CHANGELOG.md keeps
        # the URIs earlier releases shipped; overwriting through `cat` keeps
        # each file's mode.
        pattern=$(printf '%s' "$old" | sed 's/[.[\*^$]/\\&/g')
        git ls-files -z | xargs -0 grep -lF -- "$old" | grep -vx 'CHANGELOG.md' | while IFS= read -r file; do
            sed "s|$pattern|$new|g" "$file" > "$file.tmp"
            cat "$file.tmp" > "$file"
            rm "$file.tmp"
        done
    fi
    schema="${dir}package/$(field schema-artifact "$manifest")"
    old_digest=$(field schema-sha256 "$manifest")
    new_digest=$(sha256_file "$schema")
    sed "s/$old_digest/$new_digest/" "$manifest" > "$manifest.tmp"
    mv "$manifest.tmp" "$manifest"
    printf '%s -> %s\n' "$old" "$new"
done
sh scripts/check-namespace-versions.sh "$tag"
