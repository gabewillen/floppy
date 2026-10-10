#!/bin/sh
# Usage: check-release-layout.sh TAG --release-dir OUT [--commit REV] [--scx PATH]
#
# A dry run of floppy's GitHub release TAG, publishing nothing
# (docs/release.md): it checks that the release layout resolves every
# repository namespace and service the bots' documents name, from a local
# directory, exactly as `scx`'s resolver would resolve them from GitHub.
#
# OUT is a scripts/prepare-release.sh release directory. REV (default HEAD)
# is the commit the release's tag would name; its manifests and schemas are
# the ones the resolver reads. PATH is the `scx` binary (default: `scx` on
# PATH).
#
# 1. Lays out a GitHub mirror in a temporary directory, in the paths the
#    GitHub resolver component requests (scxml.rs
#    components/scxml-resolver-github): the API answers for
#    `repos/stateforward/floppy/releases/latest` (TAG) and
#    `repos/stateforward/floppy/git/ref/tags/TAG` (REV), the packages'
#    `namespaces/*/package` and `services/*/package` files at REV as raw
#    repository files, and OUT's files as the release's assets.
# 2. Serves it on loopback HTTP (python3's http.server; `scx` accepts plain
#    http only for a loopback mirror) and runs, against it with
#    `--github-api-base`, `--github-raw-base`, and `--github-download-base`
#    and an empty `--cache-dir`: `scx validate` on every bot root and ability
#    document (manifests, schemas, and package validators), then `scx run`
#    on every bot root with no input, which binds every component (verified
#    against the release's SHA256SUMS) before the session starts and then
#    closes at the end of input. A bot that loads the sql service grants it a
#    scratch `--dir`.
# 3. Checks that every repository URI the documents name was pinned to TAG
#    and REV, with a component digest SHA256SUMS lists.
set -eu

usage() {
    echo "usage: check-release-layout.sh TAG --release-dir OUT [--commit REV] [--scx PATH]" >&2
    exit 2
}

[ "$#" -ge 1 ] || usage
tag=$1
shift
case "$tag" in
    v[0-9]*) ;;
    *) echo "TAG must look like vMAJOR.MINOR.PATCH" >&2; exit 2 ;;
esac
release_dir=''
rev=HEAD
scx=scx
while [ "$#" -gt 0 ]; do
    case "$1" in
        --release-dir)
            [ "$#" -ge 2 ] && [ -n "$2" ] && [ -z "$release_dir" ] || usage
            release_dir=$2
            shift 2
            ;;
        --commit)
            [ "$#" -ge 2 ] && [ -n "$2" ] || usage
            rev=$2
            shift 2
            ;;
        --scx)
            [ "$#" -ge 2 ] && [ -n "$2" ] || usage
            scx=$2
            shift 2
            ;;
        *) usage ;;
    esac
done
[ -n "$release_dir" ] || usage
test -f "$release_dir/SHA256SUMS" || { echo "no SHA256SUMS in $release_dir" >&2; exit 1; }
release_dir=$(CDPATH='' cd -- "$release_dir" && pwd -P)
command -v python3 >/dev/null 2>&1 || { echo "check-release-layout: python3 is required" >&2; exit 1; }

workspace=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
. "$workspace/scripts/sha256.sh"
commit=$(git -C "$workspace" rev-parse --verify "$rev^{commit}")
owner_repo='stateforward/floppy'
repository="https://github.com/$owner_repo"

work=$(mktemp -d "${TMPDIR:-/tmp}/floppy-release-layout.XXXXXX")
server_pid=''
cleanup() {
    [ -z "$server_pid" ] || kill "$server_pid" 2>/dev/null || true
    rm -rf "$work"
}
trap cleanup EXIT HUP INT TERM

# 1. The mirror.
mirror="$work/mirror"
api="$mirror/api/repos/$owner_repo"
mkdir -p "$api/releases" "$api/git/ref/tags" "$mirror/raw/$owner_repo/$commit" \
    "$mirror/download/$owner_repo/releases/download/$tag"
printf '{"tag_name": "%s"}\n' "$tag" > "$api/releases/latest"
printf '{"ref": "refs/tags/%s", "object": {"sha": "%s", "type": "commit"}}\n' "$tag" "$commit" \
    > "$api/git/ref/tags/$tag"
packages=$(git -C "$workspace" ls-tree -r --name-only "$commit" -- namespaces services |
    grep -E '^(namespaces|services)/[^/]+/package/' || true)
[ -n "$packages" ] || { echo "no package files at $commit" >&2; exit 1; }
# shellcheck disable=SC2086 # one path per line, none with whitespace
git -C "$workspace" archive --format=tar "$commit" -- $packages |
    tar -xf - -C "$mirror/raw/$owner_repo/$commit"
cp "$release_dir"/* "$mirror/download/$owner_repo/releases/download/$tag/"

# 2. Serve it and resolve every document against it.
python3 -u -m http.server --bind 127.0.0.1 --directory "$mirror" 0 > "$work/server.log" 2>&1 &
server_pid=$!
port=''
tries=0
while [ -z "$port" ]; do
    port=$(sed -n 's/^Serving HTTP on 127\.0\.0\.1 port \([0-9]*\).*/\1/p' "$work/server.log")
    [ -n "$port" ] && break
    tries=$((tries + 1))
    [ "$tries" -le 50 ] || { cat "$work/server.log" >&2; echo "the mirror did not start" >&2; exit 1; }
    sleep 0.1
done
base="http://127.0.0.1:$port"
cache="$work/cache"
mirror_flags="--cache-dir $cache --github-api-base $base/api --github-raw-base $base/raw --github-download-base $base/download"

cd "$workspace"
for document in bots/*/*.scx abilities/*/*.scx abilities/behaviors/*/*.scx; do
    # shellcheck disable=SC2086 # the flags hold no whitespace
    "$scx" validate $mirror_flags "$document" > "$work/validate.log" 2>&1 || {
        cat "$work/validate.log" >&2
        echo "scx validate failed against the release layout: $document" >&2
        exit 1
    }
    printf 'validated %s\n' "$document"
done
# names_sql DOC: whether DOC, or a document it loads by a literal relative
# `<invoke src>` (recursively), names the sql service, which then needs its
# `--dir` grant (and must not get one otherwise).
names_sql() {
    case " $visited " in *" $1 "*) return 1 ;; esac
    visited="$visited $1"
    grep -qF "$repository/services/sql" "$1" && return 0
    for src in $(sed -n 's/.*<invoke [^>]*src="\([^":]*\.scx\)".*/\1/p' "$1"); do
        child="$(dirname "$1")/$src"
        [ -f "$child" ] && names_sql "$child" && return 0
    done
    return 1
}
mkdir -p "$work/memory"
for document in bots/*/*.scx; do
    visited=''
    grant=''
    if names_sql "$document"; then
        grant="--dir $repository/services/sql=$work/memory::/memory"
    fi
    status=0
    # shellcheck disable=SC2086 # the flags hold no whitespace
    "$scx" run $mirror_flags --on-eof close $grant \
        "$document" < /dev/null > "$work/run.log" 2>&1 || status=$?
    # 3: the session was open and closed at the end of input, so every
    # package bound.
    test "$status" -eq 3 || {
        cat "$work/run.log" >&2
        echo "scx run exited $status against the release layout: $document" >&2
        exit 1
    }
    printf 'bound every package of %s\n' "$document"
done

# 3. Every URI the documents name is pinned to this release.
uris=$(rg -o --no-filename "$repository/(namespaces|services)/[A-Za-z0-9._-]+" bots abilities | sort -u)
[ -n "$uris" ] || { echo "the documents name no repository URI" >&2; exit 1; }
for uri in $uris; do
    printf '%s' "$uri" > "$work/uri"
    entry="$cache/index/$(sha256_file "$work/uri").json"
    test -f "$entry" || { echo "not resolved: $uri" >&2; exit 1; }
    grep -qF "\"tag\": \"$tag\"" "$entry" && grep -qF "\"commit\": \"$commit\"" "$entry" || {
        cat "$entry" >&2
        echo "not pinned to release $tag at $commit: $uri" >&2
        exit 1
    }
    component=$(sed -n 's/.*"component-sha256": *"\([0-9a-f]*\)".*/\1/p' "$entry")
    [ -n "$component" ] && grep -q "^$component  " "$release_dir/SHA256SUMS" || {
        cat "$entry" >&2
        echo "its component is not one SHA256SUMS lists: $uri" >&2
        exit 1
    }
    printf 'resolved %s@%s component-sha256=%s\n' "$uri" "$tag" "$component"
done
printf 'release %s resolves from %s at %s (dry run, nothing published)\n' "$tag" "$release_dir" "$commit"
