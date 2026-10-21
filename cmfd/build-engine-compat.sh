#!/bin/bash
# FreeForGemPear — see LICENSE
# Linux CMFD engine for older systems: the FreeForgeMiner source pinned in cmfd/ENGINE.lock (upstream Common Foundry at
# FreeForgeMiner's pinned commit + FreeForgeMiner's patches, MIT), built inside Ubuntu 20.04 so the binaries need only
# glibc 2.31 (HiveOS images on Ubuntu 20.04 and 22.04 alike). The FreeForgeMiner release binaries are built on Ubuntu 22.04
# and need glibc 2.34. Same source, same nvcc (12.9) and flags as FreeForgeMiner's build.sh; cmfd-launch from the same tree.
#
#   cmfd/build-engine-compat.sh [OUT_DIR]        (default dist/cmfd-engine-compat/linux)
# Runs itself in docker (ubuntu:20.04) with the host CUDA toolkit mounted read-only:  CUDA=/opt/cuda-12.9 (default).
# build.sh copies the result over the release binaries when the folder exists (FFGP_CMFD_COMPAT=0 keeps the release ones).
set -euo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/ENGINE.lock"
OUT="$(realpath -m "${1:-$HERE/../dist/cmfd-engine-compat/linux}")"
WORK="$(realpath -m "${WORK:-$HERE/../dist/cmfd-src-compat}")"
CUDA="${CUDA:-/opt/cuda-12.9}"
mkdir -p "$OUT" "$WORK"

if [ "${IN_CONTAINER:-0}" != 1 ]; then
  docker run --rm -e IN_CONTAINER=1 -e WORK=/work -e HOST_UID="$(id -u)" -v "$WORK":/work -v "$OUT":/out -v "$HERE":/cmfd:ro \
    -v "$CUDA":/opt/cuda:ro ubuntu:20.04 bash /cmfd/build-engine-compat.sh /out
  echo "built (glibc <= $(objdump -T "$OUT"/cmfd-miner "$OUT"/cmfd-v4-replay "$OUT"/cmfd-launch | grep -o 'GLIBC_[0-9.]*' | sort -V | tail -1)): $OUT"
  exit 0
fi

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq && apt-get install -y -qq build-essential curl git pkg-config python3 ca-certificates >/dev/null
[ -x /root/.cargo/bin/cargo ] || curl -sSf https://sh.rustup.rs | sh -s -- -y -q --profile minimal --default-toolchain 1.98.1 >/dev/null
. /root/.cargo/env
git config --global --add safe.directory '*'
[ -d "$WORK/freeforgeminer" ] || git clone -q https://github.com/pepsykolya/freeforgeminer.git "$WORK/freeforgeminer"
git -C "$WORK/freeforgeminer" fetch -q --tags || true
git -C "$WORK/freeforgeminer" checkout -q --detach "$ENGINE_COMMIT"
FB="$WORK/freeforgeminer/build.sh"
UPSTREAM=$(sed -n 's/^UPSTREAM=//p' "$FB"); UPSTREAM_COMMIT=$(sed -n 's/^UPSTREAM_COMMIT=//p' "$FB"); CUTLASS_COMMIT=$(sed -n 's/^CUTLASS_COMMIT=//p' "$FB")
SRC="$WORK/CommonFoundry"
[ -d "$SRC" ] || git clone -q --filter=blob:none "$UPSTREAM" "$SRC"
git -C "$SRC" am --abort 2>/dev/null || true
git -C "$SRC" checkout -q --detach "$UPSTREAM_COMMIT"
git -C "$SRC" -c user.name=build -c user.email=build@local am -q "$WORK/freeforgeminer"/patches/*.patch
[ -d "$WORK/cutlass" ] || git clone -q --filter=blob:none --depth=1 --branch v3.9.2 https://github.com/NVIDIA/cutlass.git "$WORK/cutlass"
git -C "$WORK/cutlass" checkout -q --detach "$CUTLASS_COMMIT"
echo "source tree $(git -C "$SRC" rev-parse HEAD^{tree}) (FreeForgeMiner $ENGINE_VERSION)"

# GPU worker: one CUDA translation unit, CUDA runtime linked statically (flags as in FreeForgeMiner's build.sh)
/opt/cuda/bin/nvcc -O3 -std=c++17 $(for a in 70 75 80 86 89 90 120; do printf -- "--generate-code=arch=compute_%s,code=sm_%s " $a $a; done) \
  --generate-code=arch=compute_70,code=compute_70 -I"$WORK/cutlass/include" \
  "$SRC/tools/production-v4-prover/cuda/koala_four_limb_replay.cu" -o /out/cmfd-v4-replay
# pool miner and launch-beacon checker
( cd "$SRC" && export CMFD_BUILD_SOURCE_COMMIT="$(git rev-parse HEAD)" CARGO_TARGET_DIR="$WORK/target" && \
  cargo build --release -p cmfd-miner --features production-mainnet 2>&1 | tail -2 && cargo build --release -p cmfd-launch --bin cmfd-launch 2>&1 | tail -2 )
cp "$WORK/target/release/cmfd-miner" "$WORK/target/release/cmfd-launch" /out/
chown -R "${HOST_UID:-0}" /out "$WORK" 2>/dev/null || true
