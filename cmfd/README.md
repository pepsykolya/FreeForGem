# CMFD engine (Common Foundry, ForgeMatrix V4)

FreeForGemPear mines Common Foundry (CMFD) with the **FreeForgeMiner** engine, used unchanged:
<https://github.com/pepsykolya/freeforgeminer> (MIT; release and SHA-256 pinned in `ENGINE.lock`).
FreeForgeMiner is an open-source fork of the official Common Foundry miner (MIT); its changes against upstream,
including the dev-fee code, are published in its `patches/` folder.

| File | What it is |
|---|---|
| `cmfd-miner` / `cmfd-miner.exe` | pool client (`cmfd+tls://IP:PORT?pin=…`, certificate-pinned TLS), share search, 1% dev fee |
| `cmfd-v4-replay` / `cmfd-v4-replay.exe` | CUDA worker: ForgeMatrix V4 forward on int8 tensor cores, BLAKE3 digest on the GPU |
| `cmfd-launch`, `production-mainnet/` | mainnet identity / launch beacon check (Linux) |
| `production-v4-inputs.py` / `PREPARE-V4-INPUTS.ps1`, `*.json` | official download and verification of the 6.4 GB model |
| `lib/libcudart.so.12` | CUDA runtime of the Linux engine |
| `LICENSE-FreeForgeMiner`, `LICENSE-CommonFoundry`, `THIRD_PARTY_NOTICES.md` | licenses (MIT) and third-party notices |

`freeforgempear --pool cmfd+tls://…` (or `--algo cmfd`) prepares the model, starts one `cmfd-miner pool` process per GPU
from this folder and prints the usual FreeForGemPear stats line (FW/s per GPU, watts, FW/kWh, shares).
`freeforgempear --algo cmfd --self-test` runs `cmfd-v4-replay --self-test` on every selected GPU.

**Dev fee for CMFD: 1%**, inside the engine: 36 s per hour per GPU (random slot) to the developer wallet
`a6b0915b0620997b2606574726fcf05628da6dc9ae9f2ff5bffe6d0a33ea9f2c` on the pool you selected, worker `ffm-fee`,
log lines `DEV FEE | …`. The Pearl dev fee of FreeForGemPear is not run in CMFD mode.

Pools, HiveOS/Windows setup, clocks (a positive core offset is set to 0 for CMFD) and the log: `MANUAL.md`, section 13.

Building: `cmfd/fetch-engine.sh` / `cmfd\fetch-engine.ps1` put the pinned release binaries into the package
(the build scripts call them); `cmfd/build-engine.sh` rebuilds the same engine from source.

Copyright (c) 2026 FreeForgeMiner contributors; portions copyright (c) 2026 CommonFoundry contributors. MIT License,
see `LICENSE-FreeForgeMiner` and `LICENSE-CommonFoundry`. The FreeForGemPear License does not apply to this folder.
