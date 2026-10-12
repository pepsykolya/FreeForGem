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

D=dist/freeforgempear-$VERSION
rm -rf "$D"; mkdir -p "$D"
cp host/target/release/freeforgempear "$LIB" LICENSE NOTICE README.md HIVEOS.md "$D/"
tar -C dist -czf "$D-linux-x86_64.tar.gz" "freeforgempear-$VERSION"
# HiveOS custom miner package: freeforgempear-<version>.tar.gz with a top-level freeforgempear/ folder
H=dist/hive/freeforgempear
rm -rf dist/hive; mkdir -p "$H"
cp host/target/release/freeforgempear "$LIB" LICENSE NOTICE README.md HIVEOS.md hiveos/h-*.sh "$H/"
sed "s/@VERSION@/$VERSION/" hiveos/h-manifest.conf > "$H/h-manifest.conf"
chmod +x "$H"/h-*.sh "$H/freeforgempear"
tar -C dist/hive -czf "dist/freeforgempear-$VERSION.tar.gz" freeforgempear

(cd dist && sha256sum "freeforgempear-$VERSION-linux-x86_64.tar.gz" "freeforgempear-$VERSION.tar.gz" | tee SHA256SUMS)
