#!/bin/sh
# Build the Lumen build-container image.
# Usage: tools/docker/build.sh <image-tag>
set -eu
tag=$1
root=$(cd "$(dirname "$0")/../.." && pwd)
ctx=$(mktemp -d)
trap 'rm -rf "$ctx"' EXIT
cp "$root/tools/docker/Dockerfile" "$root/rust-toolchain.toml" "$ctx/"
# Sandboxed/corporate environments re-terminate TLS; pass their CA through.
if [ -n "${SSL_CERT_FILE:-}" ] && [ -f "$SSL_CERT_FILE" ]; then
    cp "$SSL_CERT_FILE" "$ctx/extra-ca.crt"
else
    : > "$ctx/extra-ca.crt"
fi
set --
[ -n "${HTTPS_PROXY:-}" ] && set -- "$@" --build-arg "HTTPS_PROXY=$HTTPS_PROXY" --build-arg "https_proxy=$HTTPS_PROXY"
[ -n "${NO_PROXY:-}" ] && set -- "$@" --build-arg "NO_PROXY=$NO_PROXY" --build-arg "no_proxy=$NO_PROXY"
exec ${DOCKER:-docker} build --network host "$@" -t "$tag" "$ctx"
