#!/bin/bash
# FreeForGemPear — see LICENSE
# Reproducible from-source build of the CMFD engine: FreeForgeMiner at the commit pinned in cmfd/ENGINE.lock, whose build.sh
# clones upstream Common Foundry at its pinned commit, applies FreeForgeMiner's published patches and builds
# cmfd-miner + cmfd-v4-replay (CUDA). Needs Rust, CUDA Toolkit 12.9, git, python3; build inside Ubuntu 22.04 for HiveOS.
# The release packages use the binaries of the pinned FreeForgeMiner release instead (cmfd/fetch-engine.sh).
set -euo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/ENGINE.lock"
WORK="${WORK:-$HERE/../dist/cmfd-src}"
[ -d "$WORK/freeforgeminer" ] || git clone -q https://github.com/pepsykolya/freeforgeminer.git "$WORK/freeforgeminer"
git -C "$WORK/freeforgeminer" checkout -q --detach "$ENGINE_COMMIT"
"$WORK/freeforgeminer/build.sh"
echo "built: $WORK/freeforgeminer/dist/cmfd-miner $WORK/freeforgeminer/dist/cmfd-v4-replay"
