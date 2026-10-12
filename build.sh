#!/bin/bash
# FreeForGemPear — see LICENSE
# Builds gpu/libpearlgpu.so and the freeforgempear binary, and packs them into dist/.
# Needs: CUDA toolkit (nvcc, 12.x), a C++17 host compiler, Rust (cargo, edition 2024), git.
set -euo pipefail
cd "$(dirname "$0")"
CUDA_HOME=${CUDA_HOME:-/usr/local/cuda}
NVCC=${NVCC:-$CUDA_HOME/bin/nvcc}
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' host/Cargo.toml)

echo "== libpearlgpu.so"
"$NVCC" -O3 -std=c++17 -gencode arch=compute_86,code=sm_86 -shared -Xcompiler -fPIC \
  -o gpu/libpearlgpu.so gpu/pearlgpu.cu -L"$CUDA_HOME/lib64/stubs" -lcuda

echo "== freeforgempear"
(cd host && cargo build --release)

D=dist/freeforgempear-$VERSION
rm -rf "$D"; mkdir -p "$D"
cp host/target/release/freeforgempear gpu/libpearlgpu.so LICENSE NOTICE README.md HIVEOS.md "$D/"
tar -C dist -czf "$D-linux-x86_64.tar.gz" "freeforgempear-$VERSION"
# HiveOS custom miner package: freeforgempear-<version>.tar.gz with a top-level freeforgempear/ folder
H=dist/hive/freeforgempear
rm -rf dist/hive; mkdir -p "$H"
cp host/target/release/freeforgempear gpu/libpearlgpu.so LICENSE NOTICE README.md HIVEOS.md hiveos/h-*.sh "$H/"
sed "s/@VERSION@/$VERSION/" hiveos/h-manifest.conf > "$H/h-manifest.conf"
chmod +x "$H"/h-*.sh "$H/freeforgempear"
tar -C dist/hive -czf "dist/freeforgempear-$VERSION.tar.gz" freeforgempear

(cd dist && sha256sum "freeforgempear-$VERSION-linux-x86_64.tar.gz" "freeforgempear-$VERSION.tar.gz" | tee SHA256SUMS)
