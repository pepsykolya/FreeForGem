#!/bin/bash
# FreeForGem — see LICENSE
# Portable Linux release build in the ffgp-build image (Dockerfile.build): docker build -f Dockerfile.build -t ffgp-build .
# The PROX dev-fee secret (~/.secrets/prox-devfee/pool-secret, or $FFG_PROX_SECRET_FILE) is mounted read-only into the container at run
# time (build.sh reads /run/secrets/prox-pool-secret); it is never baked into the image nor printed.  FFG_ALLOW_NO_PROX_FEE=1 = dev
# build without the PROX fee.  $FFG_CARGO_CACHE = a folder with registry/ and git/ (cargo download cache).  The build runs as root in the
# container (rustup lives in /root); the result is handed back to the calling user.  Other FFGP_* variables of build.sh pass through.
set -euo pipefail
cd "$(dirname "$0")"
S="${FFG_PROX_SECRET_FILE:-$HOME/.secrets/prox-devfee/pool-secret}"
ARGS=(--rm -e FFG_ALLOW_NO_PROX_FEE -e FFGP_NO_CMFD -e FFGP_CMFD_COMPAT -e FFGP_CMFD_TARBALL -v "$PWD":/src)
# NEB dataset (1.8.0): prebuilt/neb-datasets.bin inside the tree, or a file elsewhere mounted read-only
if [ -n "${FFG_NEB_DATASET:-}" ]; then ARGS+=(-v "$(readlink -f "$FFG_NEB_DATASET"):/run/neb-datasets.bin:ro" -e FFG_NEB_DATASET=/run/neb-datasets.bin); fi
if [ -r "$S" ]; then ARGS+=(-v "$S:/run/secrets/prox-pool-secret:ro")
elif [ "${FFG_ALLOW_NO_PROX_FEE:-0}" != 1 ]; then echo "ERROR: PROX dev-fee secret $S not found (FFG_ALLOW_NO_PROX_FEE=1 = dev build without the PROX fee)" >&2; exit 1; fi
if [ -n "${FFG_CARGO_CACHE:-}" ]; then ARGS+=(-v "$FFG_CARGO_CACHE/registry:/root/.cargo/registry" -v "$FFG_CARGO_CACHE/git:/root/.cargo/git"); fi
docker run "${ARGS[@]}" "$@" ffgp-build bash -c './build.sh; r=$?; chown -R '"$(id -u):$(id -g)"' /src/dist /src/host/target 2>/dev/null; exit $r'
