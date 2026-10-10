#!/bin/sh
# Run a command inside the Lumen build container with the repository mounted at /src.
# Usage: tools/docker/run.sh <image-tag> <command...>
set -eu
tag=$1; shift
root=$(cd "$(dirname "$0")/../.." && pwd)
opts="--rm --network host -v $root:/src -w /src -v lumen-cargo-registry:/opt/cargo/registry"
opts="$opts -e HOST_UID=$(id -u) -e HOST_GID=$(id -g) -e LUMEN_JOBS=${LUMEN_JOBS:-$(nproc)}"
if [ -n "${SSL_CERT_FILE:-}" ] && [ -f "$SSL_CERT_FILE" ]; then
    opts="$opts -v $SSL_CERT_FILE:/etc/ssl/certs/ca-certificates.crt:ro"
fi
[ -n "${HTTPS_PROXY:-}" ] && opts="$opts -e HTTPS_PROXY=$HTTPS_PROXY -e https_proxy=$HTTPS_PROXY"
[ -n "${NO_PROXY:-}" ] && opts="$opts -e NO_PROXY=$NO_PROXY -e no_proxy=$NO_PROXY"
[ -t 0 ] && [ -t 1 ] && opts="$opts -it"
# shellcheck disable=SC2086
exec ${DOCKER:-docker} run $opts "$tag" "$@"
