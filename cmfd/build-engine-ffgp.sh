#!/bin/bash
# FreeForGemPear — see LICENSE
# Builds cmfd-miner-ffgp: the CMFD pool client of FreeForgeMiner (release pinned in cmfd/ENGINE.lock, MIT) with the
# FreeForGemPear patch in cmfd/patches/ (MIT, same license as the engine):
#   * FFM_GPU_BACKEND=opencl + FFM_GPU_UUID: runs on a non-NVIDIA GPU (AMD OpenCL worker) without nvidia-smi.
# Steps: upstream Common Foundry at FreeForgeMiner's pinned commit + FreeForgeMiner's patches + ours, cargo build.
# Needs Rust, git, curl; build inside Ubuntu 22.04 for HiveOS.   usage: cmfd/build-engine-ffgp.sh [OUT_DIR]
set -euo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/ENGINE.lock"
OUT="${1:-$HERE/../dist/cmfd-engine-ffgp/linux}"
WORK="${WORK:-$HERE/../dist/cmfd-src}"
mkdir -p "$WORK" "$OUT"
[ -d "$WORK/freeforgeminer" ] || git clone -q https://github.com/pepsykolya/freeforgeminer.git "$WORK/freeforgeminer"
git -C "$WORK/freeforgeminer" checkout -q --detach "$ENGINE_COMMIT"
UPSTREAM=$(sed -n 's/^UPSTREAM=//p' "$WORK/freeforgeminer/build.sh")
UPSTREAM_COMMIT=$(sed -n 's/^UPSTREAM_COMMIT=//p' "$WORK/freeforgeminer/build.sh")
SRC="$WORK/CommonFoundry-ffgp"
[ -d "$SRC" ] || git clone -q --filter=blob:none "$UPSTREAM" "$SRC"
git -C "$SRC" am --abort 2>/dev/null || true
git -C "$SRC" checkout -q --detach "$UPSTREAM_COMMIT"
git -C "$SRC" -c user.name=build -c user.email=build@local am -q "$WORK/freeforgeminer"/patches/*.patch "$HERE"/patches/*.patch
( cd "$SRC" && CMFD_BUILD_SOURCE_COMMIT="$(git rev-parse HEAD)" cargo build --release -p cmfd-miner --features production-mainnet )
cp "$SRC/target/release/cmfd-miner" "$OUT/cmfd-miner-ffgp"
echo "built: $OUT/cmfd-miner-ffgp"
