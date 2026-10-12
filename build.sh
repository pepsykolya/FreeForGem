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
cp host/target/release/freeforgempear gpu/libpearlgpu.so LICENSE NOTICE README.md "$D/"
tar -C dist -czf "$D-linux-x86_64.tar.gz" "freeforgempear-$VERSION"
(cd dist && sha256sum "freeforgempear-$VERSION-linux-x86_64.tar.gz" | tee "freeforgempear-$VERSION-linux-x86_64.tar.gz.sha256")
