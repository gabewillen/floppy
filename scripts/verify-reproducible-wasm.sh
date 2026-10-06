#!/bin/sh
# Proves release wasm32-wasip2 guest bytes do not depend on the checkout
# path, the Cargo target directory, or CARGO_HOME.
#
# Usage: sh scripts/verify-reproducible-wasm.sh [package ...]
# (default package: scxml-machine, the generic machine template).
#
# The working tree is snapshotted once (so concurrent edits cannot race the
# builds) into two copies at different absolute paths. For each package it
# builds the release guest three times, one package per cargo invocation
# (as verify-wasm-capabilities.sh does), with the reproducible flags from
# scripts/reproducible-wasm-env.sh:
#
#   a. copy A, fresh target directory A;
#   b. copy A, fresh target directory B at a different depth;
#   c. copy B at a different absolute path, with a different CARGO_HOME
#      (whose registry links to the real one), a third fresh target
#      directory, offline.
#
# The three artifacts must be byte-identical, and none may contain either
# copy's path, CARGO_HOME, or HOME. Everything is created under
# $CARGO_TARGET_DIR/reproducibility (default target/reproducibility); the
# copies are removed on exit.
set -eu

workspace=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
. "$workspace/scripts/sha256.sh"
target_root=${CARGO_TARGET_DIR:-target}
case "$target_root" in
    /*) ;;
    *) target_root="$workspace/$target_root" ;;
esac
proof_root="$target_root/reproducibility"
copy_a="$proof_root/checkouts/a/scxml.rs"
copy_b="$proof_root/checkouts/elsewhere/deeper/b/scxml.rs"
alt_cargo_home="$proof_root/checkouts/cargo-home"
cargo_home=${CARGO_HOME:-${HOME:?HOME must be set}/.cargo}

if [ "$#" -eq 0 ]; then
    set -- scxml-machine
fi

rm -rf "$proof_root"
mkdir -p "$copy_a" "$copy_b" "$alt_cargo_home"
trap 'rm -rf "$proof_root/checkouts"' EXIT HUP INT TERM

# Snapshot the working tree (not just committed files) so uncommitted
# sources are compared like-for-like; build output and VCS metadata are
# excluded.
(cd "$workspace" && tar -cf - --exclude=./target --exclude=./.git --exclude=./.claude --exclude=./.pi --exclude=./.commandcode --exclude=./handoff.md --exclude=node_modules --exclude='./hosts/scxml-host-browser/dist' .) \
    | (cd "$copy_a" && tar -xf -)
(cd "$copy_a" && tar -cf - .) | (cd "$copy_b" && tar -xf -)
ln -s "$cargo_home/registry" "$alt_cargo_home/registry"

build() {
    # $1 checkout, $2 target dir, $3 CARGO_HOME, remaining: cargo args
    checkout=$1
    target_dir=$2
    home_dir=$3
    shift 3
    (
        cd "$checkout"
        workspace=$(pwd -P)
        CARGO_HOME=$home_dir
        export CARGO_HOME
        . "$workspace/scripts/reproducible-wasm-env.sh"
        CARGO_TARGET_DIR=$target_dir cargo build --locked --release --target wasm32-wasip2 "$@"
    )
}

# One cargo invocation per package, exactly as verify-wasm-capabilities.sh
# builds them, so dependency feature unification cannot differ.
for package in "$@"; do
    build "$copy_a" "$proof_root/a" "$cargo_home" -p "$package"
    build "$copy_a" "$proof_root/b/nested/target" "$cargo_home" -p "$package"
    build "$copy_b" "$proof_root/c" "$alt_cargo_home" --offline -p "$package"
done

status=0
for package in "$@"; do
    artifact=$(printf '%s' "$package" | tr '-' '_').wasm
    a="$proof_root/a/wasm32-wasip2/release/$artifact"
    b="$proof_root/b/nested/target/wasm32-wasip2/release/$artifact"
    c="$proof_root/c/wasm32-wasip2/release/$artifact"
    digest_a=$(sha256_file "$a")
    digest_b=$(sha256_file "$b")
    digest_c=$(sha256_file "$c")
    printf '%s target-a sha256=%s\n%s target-b sha256=%s\n%s relocated sha256=%s\n' \
        "$package" "$digest_a" "$package" "$digest_b" "$package" "$digest_c"
    if [ "$digest_a" != "$digest_b" ] || [ "$digest_a" != "$digest_c" ]; then
        echo "non-reproducible guest bytes: $package" >&2
        status=1
    fi
    for leaked in "$copy_a" "$copy_b" "$cargo_home" "$alt_cargo_home" "$HOME/"; do
        if LC_ALL=C grep -qF "$leaked" "$a" "$c"; then
            echo "guest bytes embed a build path ($leaked): $package" >&2
            status=1
        fi
    done
done
exit "$status"
