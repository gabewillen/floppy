#!/bin/bash
# Cloud-environment setup script for floppy: installs sccache so every
# cloud session shares one remote Rust compile cache.
#
# The cache is a Garage (S3-compatible) bucket on lab, published through the
# Cloudflare tunnel as s3.willen.dev. Pair this script with these environment
# variables in the same cloud environment (the key pair is in
# ~/.config/floppy-sccache.env on Gabe's Mac, scoped to the one bucket):
#   RUSTC_WRAPPER=sccache
#   CARGO_INCREMENTAL=0
#   SCCACHE_BUCKET=floppy-sccache
#   SCCACHE_ENDPOINT=https://s3.willen.dev
#   SCCACHE_REGION=garage
#   SCCACHE_S3_USE_SSL=true
#   AWS_ACCESS_KEY_ID=...
#   AWS_SECRET_ACCESS_KEY=...
# Allowed domains: s3.willen.dev, plus github.com,
# release-assets.githubusercontent.com and objects.githubusercontent.com for
# the prebuilt binary.
set -euo pipefail

version=0.18.0
if ! command -v sccache >/dev/null 2>&1; then
    url="https://github.com/mozilla/sccache/releases/download/v$version/sccache-v$version-x86_64-unknown-linux-musl.tar.gz"
    if curl -fsSL "$url" | tar -xz -C /tmp; then
        install -m 0755 "/tmp/sccache-v$version-x86_64-unknown-linux-musl/sccache" /usr/local/bin/sccache
    else
        # Fallback when GitHub downloads are blocked: build from crates.io (a few minutes).
        cargo install --locked --version "$version" --root /usr/local sccache
    fi
fi
sccache --version
