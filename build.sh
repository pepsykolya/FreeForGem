#!/bin/bash
# FreeForGemPear — see LICENSE
# Builds the freeforgempear host binary, links the prebuilt GPU library (prebuilt/linux-x86_64/libpearlgpu.so; the kernel is
# closed source) and packs both into dist/.  No nvcc needed; the linker must resolve the library's libcuda.so.1 (NVIDIA driver or CUDA stubs).
# Needs: a C compiler/linker (gcc), Rust (cargo, edition 2024), git.
set -euo pipefail
cd "$(dirname "$0")"
LIB=prebuilt/linux-x86_64/libpearlgpu.so
[ -f "$LIB" ] || { echo "missing $LIB (prebuilt GPU library)"; exit 1; }
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' host/Cargo.toml)

echo "== freeforgempear (links $LIB)"
(cd host && cargo build --release)

# CMFD engine (FreeForgeMiner release pinned in cmfd/ENGINE.lock, MIT) goes into the packages as cmfd/; FFGP_NO_CMFD=1 = Pearl only
E=dist/cmfd-engine/linux
[ "${FFGP_NO_CMFD:-0}" = 1 ] || cmfd/fetch-engine.sh "$E"
# Linux engine binaries rebuilt from the same pinned source on Ubuntu 20.04 (glibc 2.31; the release ones need 2.34), made by
# cmfd/build-engine-compat.sh; FFGP_CMFD_COMPAT=0 keeps the FreeForgeMiner release binaries.
C=dist/cmfd-engine-compat/linux
if [ "${FFGP_NO_CMFD:-0}" != 1 ] && [ "${FFGP_CMFD_COMPAT:-1}" != 0 ]; then
  if [ -x "$C/cmfd-miner" ] && [ -x "$C/cmfd-v4-replay" ] && [ -x "$C/cmfd-launch" ]; then
    cp "$C"/cmfd-miner "$C"/cmfd-v4-replay "$C"/cmfd-launch "$E/"
    g=$(objdump -T "$E"/cmfd-miner "$E"/cmfd-v4-replay "$E"/cmfd-launch | grep -o 'GLIBC_[0-9.]*' | sort -V | tail -1)
    { echo "linux binaries: built on Ubuntu 20.04 from the FreeForgeMiner source at commit $(. cmfd/ENGINE.lock; echo $ENGINE_COMMIT) (cmfd/build-engine-compat.sh), need $g"
      (cd "$E" && sha256sum cmfd-miner cmfd-v4-replay cmfd-launch); } >> "$E/ENGINE"
    echo "CMFD engine: Ubuntu 20.04 build from $C ($g)"
  else
    echo "WARNING: $C missing: the CMFD engine in the package needs glibc 2.34+ (run cmfd/build-engine-compat.sh first)"
  fi
fi

D=dist/freeforgempear-$VERSION
rm -rf "$D"; mkdir -p "$D"
cp host/target/release/freeforgempear "$LIB" LICENSE NOTICE README.md HIVEOS.md "$D/"
[ "${FFGP_NO_CMFD:-0}" = 1 ] || cp -a "$E" "$D/cmfd"
tar -C dist -czf "$D-linux-x86_64.tar.gz" "freeforgempear-$VERSION"
# HiveOS custom miner package: freeforgempear-<version>.tar.gz with a top-level freeforgempear/ folder.
# HiveOS splits the file name at its LAST '-' into miner name / version, so <version> must contain no '-'
# (1.5.0rc1, not 1.5.0-rc1) and the folder must be exactly the miner name.
H=dist/hive/freeforgempear
rm -rf dist/hive; mkdir -p "$H"
cp host/target/release/freeforgempear "$LIB" LICENSE NOTICE README.md HIVEOS.md hiveos/h-*.sh "$H/"
sed "s/@VERSION@/$VERSION/" hiveos/h-manifest.conf > "$H/h-manifest.conf"
[ "${FFGP_NO_CMFD:-0}" = 1 ] || cp -a "$E" "$H/cmfd"
chmod +x "$H"/h-*.sh "$H/freeforgempear"
tar -C dist/hive -czf "dist/freeforgempear-$VERSION.tar.gz" freeforgempear

(cd dist && sha256sum "freeforgempear-$VERSION-linux-x86_64.tar.gz" "freeforgempear-$VERSION.tar.gz" | tee SHA256SUMS)
