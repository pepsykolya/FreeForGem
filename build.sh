#!/bin/bash
# FreeForGem — see LICENSE
# Builds the freeforgem host binary and packs it with the prebuilt GPU libraries (prebuilt/linux-x86_64/libpearlgpu.so for Pearl,
# libnoidgpu.so for NOID; the kernels are closed source) into dist/.  No nvcc needed; the linker must resolve the library's libcuda.so.1 (NVIDIA driver or CUDA stubs).
# Needs: a C compiler/linker (gcc), Rust (cargo, edition 2024), git.
set -euo pipefail
cd "$(dirname "$0")"
LIB=prebuilt/linux-x86_64/libpearlgpu.so
NLIB=prebuilt/linux-x86_64/libnoidgpu.so
[ -f "$LIB" ] || { echo "missing $LIB (prebuilt GPU library)"; exit 1; }
[ -f "$NLIB" ] || { echo "missing $NLIB (prebuilt NOID GPU library)"; exit 1; }
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' host/Cargo.toml)

echo "== freeforgem"
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

D=dist/freeforgem-$VERSION
rm -rf "$D"; mkdir -p "$D"
cp host/target/release/freeforgem "$LIB" "$NLIB" LICENSE NOTICE README.md HIVEOS.md MANUAL.md "$D/"
[ "${FFGP_NO_CMFD:-0}" = 1 ] || cp -a "$E" "$D/cmfd"
tar -C dist -czf "$D-linux-x86_64.tar.gz" "freeforgem-$VERSION"
# HiveOS custom miner package: freeforgem-<version>.tar.gz with a top-level freeforgem/ folder.
# HiveOS splits the file name at its LAST '-' into miner name / version, so <version> must contain no '-'
# (1.7.0beta, not 1.7.0-beta: the '-' is dropped here) and the folder must be exactly the miner name.
HV=${VERSION//-/}
H=dist/hive/freeforgem
rm -rf dist/hive; mkdir -p "$H"
cp host/target/release/freeforgem "$LIB" "$NLIB" LICENSE NOTICE README.md HIVEOS.md MANUAL.md hiveos/h-*.sh "$H/"
sed "s/@VERSION@/$HV/" hiveos/h-manifest.conf > "$H/h-manifest.conf"
[ "${FFGP_NO_CMFD:-0}" = 1 ] || cp -a "$E" "$H/cmfd"
chmod +x "$H"/h-*.sh "$H/freeforgem"
tar -C dist/hive -czf "dist/freeforgem-$HV.tar.gz" freeforgem

(cd dist && sha256sum "freeforgem-$VERSION-linux-x86_64.tar.gz" "freeforgem-$HV.tar.gz" | tee SHA256SUMS)
