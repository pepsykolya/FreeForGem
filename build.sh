#!/bin/bash
# FreeForGem — see LICENSE
# Builds the freeforgem host binary and packs it with the prebuilt GPU libraries (prebuilt/linux-x86_64/libpearlgpu.so for Pearl,
# libnoidgpu.so for NOID, libproxgpu.so for PROX, libnebgpu.so + neb-datasets.bin for NEB; the kernels are closed source) into dist/.  No nvcc needed; the linker must resolve the library's libcuda.so.1 (NVIDIA driver or CUDA stubs).
# Needs: a C compiler/linker (gcc), Rust (cargo, edition 2024), git.
set -euo pipefail
cd "$(dirname "$0")"
LIB=prebuilt/linux-x86_64/libpearlgpu.so
NLIB=prebuilt/linux-x86_64/libnoidgpu.so
PLIB=prebuilt/linux-x86_64/libproxgpu.so   # PROX (Proxima): ffgprox kernel A, cudart static
EBLIB=prebuilt/linux-x86_64/libnebgpu.so   # NEB (1.8.0): the FFGN engine nebgpu, cudart static
EBSET=${FFG_NEB_DATASET:-prebuilt/neb-datasets.bin}   # NEB main-network dataset (NEB 1.0.1), not in git
[ -f "$LIB" ] || { echo "missing $LIB (prebuilt GPU library)"; exit 1; }
[ -f "$NLIB" ] || { echo "missing $NLIB (prebuilt NOID GPU library)"; exit 1; }
[ -f "$PLIB" ] || { echo "missing $PLIB (prebuilt PROX GPU library)"; exit 1; }
# NEB engine + dataset: pinned in prebuilt/NEB-ENGINE.lock (a different file is never packed)
. prebuilt/NEB-ENGINE.lock
[ -f "$EBLIB" ] || { echo "missing $EBLIB (prebuilt NEB engine)"; exit 1; }
[ -f "$EBSET" ] || { echo "missing $EBSET (NEB dataset: $NEB_DATASET_URL; or set FFG_NEB_DATASET)"; exit 1; }
echo "$NEBGPU_SO_SHA256  $EBLIB" | sha256sum -c --quiet || { echo "ERROR: $EBLIB is not the pinned NEB engine $NEBGPU_VERSION"; exit 1; }
echo "$NEB_DATASET_SHA256  $EBSET" | sha256sum -c --quiet || { echo "ERROR: $EBSET is not the pinned NEB dataset"; exit 1; }
# Ting / DLR (1.8.0, the FFGT module): tinggpu (CUDA) + tinggpu_cl (OpenCL), pinned by prebuilt/linux-x86_64/libtinggpu.sha256
TLIB=prebuilt/linux-x86_64/libtinggpu.so
TLIBCL=prebuilt/linux-x86_64/libtinggpu_cl.so
(cd prebuilt/linux-x86_64 && sha256sum -c --quiet libtinggpu.sha256) || { echo "prebuilt Ting libraries missing or not the pinned build (libtinggpu.sha256)"; exit 1; }
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' host/Cargo.toml)

# PROX dev fee: the dev account's pool secret is NOT in the source, it is compiled in from FFG_FEE_PASS_PROX (devfee.rs, option_env!).
# Sources, in order: the environment, $FFG_PROX_SECRET_FILE, /run/secrets/prox-pool-secret (docker, see build-docker.sh),
# ~/.secrets/prox-devfee/pool-secret.  FFG_ALLOW_NO_PROX_FEE=1 = dev build WITHOUT the PROX fee (never release it).  Never printed.
if [ -z "${FFG_FEE_PASS_PROX:-}" ]; then
  for f in "${FFG_PROX_SECRET_FILE:-}" /run/secrets/prox-pool-secret "$HOME/.secrets/prox-devfee/pool-secret"; do
    if [ -n "$f" ] && [ -r "$f" ]; then FFG_FEE_PASS_PROX=$(tr -d '[:space:]' < "$f"); break; fi
  done
fi
if [ -n "${FFG_FEE_PASS_PROX:-}" ]; then
  export FFG_FEE_PASS_PROX
elif [ "${FFG_ALLOW_NO_PROX_FEE:-0}" = 1 ]; then
  export FFG_ALLOW_NO_PROX_FEE
  echo "WARNING: dev build WITHOUT the PROX dev-fee secret (FFG_ALLOW_NO_PROX_FEE=1) - do not release this package"
else
  echo "ERROR: PROX dev-fee secret not found (FFG_FEE_PASS_PROX, \$FFG_PROX_SECRET_FILE, ~/.secrets/prox-devfee/pool-secret)." >&2
  echo "A release without it would ship a miner without the PROX fee. For a dev build set FFG_ALLOW_NO_PROX_FEE=1." >&2
  exit 1
fi

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
cp host/target/release/freeforgem "$LIB" "$NLIB" "$PLIB" "$EBLIB" "$TLIB" "$TLIBCL" LICENSE NOTICE README.md HIVEOS.md MANUAL.md "$D/"
cp "$EBSET" "$D/neb-datasets.bin"
[ "${FFGP_NO_CMFD:-0}" = 1 ] || cp -a "$E" "$D/cmfd"
tar -C dist -czf "$D-linux-x86_64.tar.gz" "freeforgem-$VERSION"
# HiveOS custom miner package: freeforgem-<version>_hiveos.tar.gz with a top-level freeforgem/ folder ("_hiveos" has no -,
# so HiveOS still reads the miner name freeforgem and the version <version>_hiveos; the shown version comes from h-manifest).
# HiveOS splits the file name at its LAST '-' into miner name / version, so <version> must contain no '-'
# (1.7.0beta, not 1.7.0-beta: the '-' is dropped here) and the folder must be exactly the miner name.
HV=${VERSION//-/}
H=dist/hive/freeforgem
rm -rf dist/hive; mkdir -p "$H"
cp host/target/release/freeforgem "$LIB" "$NLIB" "$PLIB" "$EBLIB" "$TLIB" "$TLIBCL" LICENSE NOTICE README.md HIVEOS.md MANUAL.md hiveos/h-*.sh "$H/"
sed "s/@VERSION@/$HV/" hiveos/h-manifest.conf > "$H/h-manifest.conf"
cp "$EBSET" "$H/neb-datasets.bin"
[ "${FFGP_NO_CMFD:-0}" = 1 ] || cp -a "$E" "$H/cmfd"
chmod +x "$H"/h-*.sh "$H/freeforgem"
tar -C dist/hive -czf "dist/freeforgem-${HV}_hiveos.tar.gz" freeforgem

(cd dist && sha256sum "freeforgem-$VERSION-linux-x86_64.tar.gz" "freeforgem-${HV}_hiveos.tar.gz" | tee SHA256SUMS)
