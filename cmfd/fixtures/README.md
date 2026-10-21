# CMFD test vector

`mainnet-template-nonce0-count32.coefficients.bin` — replay coefficients of the Common Foundry mainnet template at nonce 0
for 32 nonces, from the qualification fixtures of the official AMD HIP miner in Common Foundry v1.0.16
(`share/common-foundry/amd-hip/fixtures/`, https://github.com/Common-Foundry-1/CommonFoundry/releases/tag/v1.0.16).
MIT License, Copyright (c) 2026 Common Foundry AMD HIP contributors (`LICENSE-CommonFoundry-AMD-HIP`).
SHA-256 of the file: `8091b55e1bdbc3813b3908d29709096b19b11da1244242c267890e11488304d3`.

One `RUNBATCH 32` of these coefficients on the ForgeMatrix V4 model must produce final activations (64 MiB) with SHA-256
`fe67b80af4671210cd33306d0976c7dbb285dd55203d8eff733866cd7ef4a7a7` (their qualification script `run_batch32_qualification.sh`).
`freeforgempear --algo cmfd --self-test` runs it twice per GPU when the model is present.
