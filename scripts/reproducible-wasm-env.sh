# shellcheck shell=sh
# Sourced (not executed) by scripts that build release wasm32-wasip2 guests
# (adapted from scxml.rs/scripts/reproducible-wasm-env.sh: the workspace
# prefix is /nemobot).
#
# Contract: after sourcing with `workspace` set to the absolute repository
# root, `CARGO_TARGET_WASM32_WASIP2_RUSTFLAGS` remaps every absolute build
# path that rustc can embed into guest bytes (panic locations and file
# names) to a fixed virtual prefix:
#
#   <workspace>                          -> /nemobot
#   <CARGO_HOME>/registry/src            -> /cargo/registry/src
#   <CARGO_HOME>/git/checkouts           -> /cargo/git/checkouts
#   <sysroot>/lib/rustlib/src/rust       -> /rustc/<commit-hash>
#
# The last mapping makes a toolchain with the `rust-src` component installed
# (which otherwise embeds the local std source path) emit the same bytes as
# a minimal toolchain without it. With identical sources, Cargo.lock, and
# toolchain, the release guest bytes are therefore independent of the
# checkout path, the target directory, the user's home directory, and
# whether `rust-src` is installed.
#
# The same target rustflags also pass `-Dwarnings`, so every release guest
# build fails on any rustc warning in workspace code (Cargo caps lints for
# registry dependencies, and lint levels do not change emitted bytes).
#
# The flags are scoped to the wasm32-wasip2 target, so host tools and build
# scripts compile with unchanged flags. A caller-set RUSTFLAGS or
# CARGO_ENCODED_RUSTFLAGS would silently take precedence over target
# rustflags, so either one fails closed here. Paths containing whitespace
# cannot be expressed in a whitespace-split flag list and also fail closed.

reproducible_wasm_fail() {
    echo "reproducible-wasm-env: $1" >&2
    exit 1
}

[ -n "${workspace:-}" ] || reproducible_wasm_fail 'workspace must be set before sourcing'
[ -z "${RUSTFLAGS:-}" ] || reproducible_wasm_fail 'RUSTFLAGS would override reproducible target rustflags; unset it'
[ -z "${CARGO_ENCODED_RUSTFLAGS:-}" ] || reproducible_wasm_fail 'CARGO_ENCODED_RUSTFLAGS would override reproducible target rustflags; unset it'
[ -z "${CARGO_TARGET_WASM32_WASIP2_RUSTFLAGS:-}" ] || reproducible_wasm_fail 'CARGO_TARGET_WASM32_WASIP2_RUSTFLAGS is already set; unset it'

reproducible_cargo_home=${CARGO_HOME:-${HOME:?HOME must be set}/.cargo}
reproducible_sysroot=$(rustc --print sysroot) || reproducible_wasm_fail 'rustc --print sysroot failed'
reproducible_commit=$(rustc -vV | sed -n 's/^commit-hash: //p')
case "$reproducible_commit" in
    ''|*[!0-9a-f]*) reproducible_wasm_fail 'rustc -vV reported no commit hash' ;;
esac
for reproducible_path in "$workspace" "$reproducible_cargo_home" "$reproducible_sysroot"; do
    case "$reproducible_path" in
        /*) ;;
        *) reproducible_wasm_fail "build path is not absolute: $reproducible_path" ;;
    esac
    case "$reproducible_path" in
        *[[:space:]]*) reproducible_wasm_fail "build path contains whitespace: $reproducible_path" ;;
    esac
done

# rustc applies the last matching mapping, so the most specific prefixes
# come last.
CARGO_TARGET_WASM32_WASIP2_RUSTFLAGS="-Dwarnings --remap-path-prefix=$workspace=/nemobot --remap-path-prefix=$reproducible_cargo_home/registry/src=/cargo/registry/src --remap-path-prefix=$reproducible_cargo_home/git/checkouts=/cargo/git/checkouts --remap-path-prefix=$reproducible_sysroot/lib/rustlib/src/rust=/rustc/$reproducible_commit"
export CARGO_TARGET_WASM32_WASIP2_RUSTFLAGS
unset reproducible_cargo_home reproducible_sysroot reproducible_commit reproducible_path
