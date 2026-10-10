#!/bin/sh
# Usage: prepare-release.sh TAG --artifacts-dir DIR --release-dir OUT
#
# Prepares the assets of floppy's GitHub release TAG in OUT, and publishes
# nothing (docs/release.md). DIR is a scripts/build-components.sh artifacts
# directory; for a release it is the canonical build's
# (scripts/build-canonical-components.sh).
#
# 1. scripts/check-package-versions.sh TAG: every package is release TAG.
# 2. Copies into OUT the release assets the packages' manifests name, which
#    scx's resolver fetches from the release (scxml.rs
#    docs/executable-content-extensions.md and docs/invoked-services.md,
#    "Loading ... from a repository"): each namespace's
#    `extension-artifact` (from DIR/namespaces), each service's
#    `service-artifact` and `validator-artifact` (from DIR/services). A
#    package validator's bytes must be exactly the manifest's
#    `validator-sha256` (D27: a release records its own build's digest), or
#    the release would not resolve, so a mismatch fails closed.
# 3. Writes OUT/SHA256SUMS (`sha256sum` format) listing every asset of step
#    2, which the resolver authenticates each component against, and copies
#    LICENSE-APACHE and LICENSE-MIT.
#
# OUT must not exist yet or be empty. The manifests and schemas are not
# assets: the resolver reads them from the repository at the release's
# commit.
set -eu

usage() {
    echo "usage: prepare-release.sh TAG --artifacts-dir DIR --release-dir OUT" >&2
    exit 2
}

[ "$#" -ge 1 ] || usage
tag=$1
shift
artifacts_dir=''
release_dir=''
while [ "$#" -gt 0 ]; do
    case "$1" in
        --artifacts-dir)
            [ "$#" -ge 2 ] && [ -n "$2" ] && [ -z "$artifacts_dir" ] || usage
            artifacts_dir=$2
            shift 2
            ;;
        --release-dir)
            [ "$#" -ge 2 ] && [ -n "$2" ] && [ -z "$release_dir" ] || usage
            release_dir=$2
            shift 2
            ;;
        *) usage ;;
    esac
done
[ -n "$artifacts_dir" ] && [ -n "$release_dir" ] || usage

workspace=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
. "$workspace/scripts/sha256.sh"
sh "$workspace/scripts/check-package-versions.sh" "$tag"

test -d "$artifacts_dir/namespaces" && test -d "$artifacts_dir/services" || {
    echo "not a build-components.sh artifacts directory: $artifacts_dir" >&2
    exit 1
}
if [ -e "$release_dir" ]; then
    [ -d "$release_dir" ] && [ -z "$(ls -A "$release_dir")" ] || {
        echo "the release directory exists and is not empty: $release_dir" >&2
        exit 1
    }
fi
mkdir -p "$release_dir"

field() {
    sed -n "s/^  \"$1\": \"\\(.*\\)\",\\{0,1\\}\$/\\1/p" "$2"
}

# copy_asset NAME FROM: copies the asset NAME from the directory FROM.
copy_asset() {
    case "$1" in
        ''|*/*|.|..) echo "not a plain release asset name: \`$1\`" >&2; exit 1 ;;
    esac
    test ! -e "$release_dir/$1" || { echo "two packages name the asset \`$1\`" >&2; exit 1; }
    test -f "$2/$1" || { echo "missing component: $2/$1" >&2; exit 1; }
    cp "$2/$1" "$release_dir/$1"
}

for manifest in "$workspace"/namespaces/*/package/*.extension.json; do
    copy_asset "$(field extension-artifact "$manifest")" "$artifacts_dir/namespaces"
done
for manifest in "$workspace"/services/*/package/*.service.json; do
    copy_asset "$(field service-artifact "$manifest")" "$artifacts_dir/services"
    validator=$(field validator-artifact "$manifest")
    [ -n "$validator" ] || continue
    copy_asset "$validator" "$artifacts_dir/services"
    pinned=$(field validator-sha256 "$manifest")
    built=$(sha256_file "$release_dir/$validator")
    test "$built" = "$pinned" || {
        echo "$validator is $built, but ${manifest#"$workspace"/} pins validator-sha256 $pinned:" >&2
        echo "build with scripts/build-canonical-components.sh and record its digest in the manifest" >&2
        exit 1
    }
done

(
    cd "$release_dir"
    sums=SHA256SUMS.tmp
    : > "$sums"
    for asset in *.wasm; do
        printf '%s  %s\n' "$(sha256_file "$asset")" "$asset" >> "$sums"
    done
    mv "$sums" SHA256SUMS
)
cp "$workspace/LICENSE-APACHE" "$workspace/LICENSE-MIT" "$release_dir/"
cat "$release_dir/SHA256SUMS"
printf 'release %s prepared in %s (not published)\n' "$tag" "$release_dir"
