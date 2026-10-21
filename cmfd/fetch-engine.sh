#!/bin/bash
# FreeForGemPear — see LICENSE
# Puts the CMFD engine (FreeForgeMiner release pinned in cmfd/ENGINE.lock, MIT) into DEST (default: dist/cmfd-engine/linux):
# downloads the release tarball, checks its SHA-256, unpacks only the engine files (not FreeForgeMiner's own HiveOS scripts).
#   cmfd/fetch-engine.sh [DEST]          $FFGP_CMFD_TARBALL=<local copy> skips the download (the hash is still checked)
set -euo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/ENGINE.lock"
DEST="${1:-$HERE/../dist/cmfd-engine/linux}"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
TAR="${FFGP_CMFD_TARBALL:-$TMP/engine.tar.gz}"
[ -f "$TAR" ] || curl -fsSL --retry 3 -o "$TAR" "$LINUX_URL"
echo "$LINUX_SHA256  $TAR" | sha256sum -c --quiet - || { echo "CMFD engine: SHA-256 mismatch for $TAR"; exit 1; }
tar -xzf "$TAR" -C "$TMP"
S="$TMP/freeforgeminer"
rm -rf "$DEST"; mkdir -p "$DEST/lib" "$DEST/production-mainnet"
cp "$S"/cmfd-miner "$S"/cmfd-v4-replay "$S"/cmfd-launch "$S"/production-v4-inputs.py "$S"/V4-INPUT-CHUNKS.json \
   "$S"/production-v4-rcnet-1-inputs.json "$S"/FORGEMATRIX-V4-FIXED-ARTIFACT-RECORD-V1.json "$S"/THIRD_PARTY_NOTICES.md \
   "$S"/LICENSE-CommonFoundry "$DEST/"
cp "$S"/LICENSE "$DEST/LICENSE-FreeForgeMiner"
cp "$S"/lib/* "$DEST/lib/"
cp -p "$S"/production-mainnet/* "$DEST/production-mainnet/"
cp "$HERE/README.md" "$DEST/README.md"
mkdir -p "$DEST/fixtures"; cp "$HERE"/fixtures/* "$DEST/fixtures/"   # official test vector for --self-test (MIT, CF 1.0.16)
printf '%s %s\nsource %s\nsha256 %s\n' "$ENGINE_NAME" "$ENGINE_VERSION" "$LINUX_URL" "$LINUX_SHA256" > "$DEST/ENGINE"
chmod +x "$DEST"/cmfd-miner "$DEST"/cmfd-v4-replay "$DEST"/cmfd-launch
echo "CMFD engine $ENGINE_NAME $ENGINE_VERSION -> $DEST"
