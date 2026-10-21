# CMFD (Common Foundry) in FreeForGemPear — design and journal

Branch `cmfd` (from `rc-1.5.4`), version 1.6.0, **not released, not pushed**.

## Design

**What CMFD needs.** Pool protocol `cmfd+tls://IPv4:PORT?pin=<64 hex>`: TLS with a pinned pool certificate, address-only
payout (64-hex x-only key), worker name; jobs are ProductionV4 block challenges, the miner searches nonces: each nonce =
one forward ("FW") of the 6.4 GB ForgeMatrix V4 model (`MODEL-V2.bank`, 3 banks × 128 layers, int8 GEMM + reduction),
the share is the final-activation digest under the pool target; the pool re-verifies every share with its own replay worker.
The GPU part (`cmfd-v4-replay`, CUDA, ~1.5k lines + fused kernel) talks to the pool client (`cmfd-miner`, Rust, 7.6k lines
on top of 187k lines of `cmfd-consensus`/`cmfd-node`) over a line protocol (`RUNBATCHD n coef out` → `CMFD_V4_REPLAY_DONE`).
FreeForgeMiner = upstream Common Foundry (MIT) at a pinned commit + 17 published patches (tensor-core path, GPU digest,
fused kernel, 1% dev fee, integrity watchdog, share classification, batch, …).

**Chosen integration: FreeForgeMiner as an unchanged engine, FreeForGemPear as the front end.**
Re-implementing the CMFD pool client and the consensus checks in the FFGP host would mean porting a large, fast-moving
consensus code base (and owning its bugs); linking it as a crate would drag the whole CommonFoundry workspace (Plonky3 etc.)
into the host build. Instead:

* `host/src/cmfd.rs`: `--algo cmfd` (default for a `cmfd+tls://` pool URL; `--algo pearl` with such a URL is an error).
  Validates pool/wallet/worker, prepares the model with the engine's official script (Linux `production-v4-inputs.py` +
  `cmfd-miner mainnet-launch-info` + `cmfd-launch fetch`; Windows `PREPARE-V4-INPUTS.ps1`), starts **one `cmfd-miner pool`
  per GPU** (GPU by UUID from the CUDA ordinal, `CUDA_VISIBLE_DEVICES=<uuid>`, `FFM_SINGLE_WORKER`, `CMFD_MODE`), shows its
  output with a `[GPUn]` prefix, parses `MINER STATS`, prints the FFGP stats line every 10 s:
  `[ 290s] GPU0 37.04 (190 W, 701390 FW/kWh) | total 37.04 FW/s | … | user sub 84 acc 80 rej 0 | stale 4 | height 25089 [DEVFEE]`.
  Restarts an exited engine with back-off 15/30/60/120/300 s; after 5 failed starts in a row (<120 s each) the GPU is
  left alone (no reconnect storm — pools ban for that); all GPUs failed → exit with an error. Hints in the log for
  "out of memory" and for GPU faults (illegal memory access / INTEGRITY ERROR → offset/undervolt). Ctrl-C/SIGTERM →
  SIGINT to the engines (graceful), 10 s, then kill; second Ctrl-C kills every engine at once.
* Shared with Pearl: CLI (`--pool/--wallet/--worker/--devices/--minutes`), clock locks (`--lock-cclock/--lock-mclock/--moffset`,
  applied before the engines start, undone on exit), NVML power in the stats line, HiveOS package, Windows launcher.
  Not shared: `--auto-uv/--uv-offset` (Pearl-only: the tuner validates with the Pearl kernel; CMFD is undervolt-sensitive) —
  warned and ignored. New options: `--cmfd-mode speed|eco`, `--cmfd-batch N`, `--cmfd-model DIR`, `--cmfd-dir DIR`,
  `--per-gpu-workers`, `--cmfd-skip-prepare`; env `FFGP_CMFD_DIR`, `FFGP_CMFD_MODEL`, `FFGP_CMFD_WORK`.
* **GPU code**: FreeForgeMiner 1.2.3 binaries, unchanged, in `cmfd/` of the package; pinned by URL + SHA-256 in
  `cmfd/ENGINE.lock` (`cmfd/fetch-engine.sh|.ps1`, called by `build.sh`/`build-windows.ps1`; `FFGP_NO_CMFD=1` = Pearl-only build);
  `cmfd/build-engine.sh` rebuilds the same engine from source (FreeForgeMiner commit eca7015 → its `build.sh`).
  MIT notices: `cmfd/LICENSE-FreeForgeMiner`, `cmfd/LICENSE-CommonFoundry`, `THIRD_PARTY_NOTICES.md`, NOTICE section;
  the FreeForGemPear License explicitly does not apply to `cmfd/`.
* **Dev fee**: not shared with Pearl. CMFD uses only the engine's built-in 1% (36 s per hour per GPU, random slot,
  same pool, worker `ffm-fee`, wallet `a6b0915b…9f2c`); FFGP's Pearl fee (`ffgp-devfee`, PRL wallet) is not started in
  CMFD mode — total stays 1%. (A CMFD wallet cannot receive the PRL fee anyway.)
* **Model**: default `/hive/miners/custom/cmfd-model` on HiveOS (shared with FreeForgeMiner: one 6.4 GB download per rig),
  else `cmfd-model/` next to the binary. Engine scratch: `FFGP_CMFD_WORK`, on HiveOS `/hive/miners/custom/freeforgempear-data/cmfd-work`
  (not `/hive-config`, which is flash), else `cmfd-work/` next to the binary.
* **HiveOS**: coin follows the flight-sheet pool URL; `h-run.sh` disables globbing (`?` in the URL); `h-stats.sh` reads
  `total … TH/s` (Pearl, unchanged output) or `total … FW/s` (CMFD: `hs_units:"hs"`, `khs = FW/s/1000`, `algo:"forgematrix_v4"`,
  same as FreeForgeMiner). `CUSTOM_ALGO` stays `pearlhash`; the flight sheet sets *Hash algorithm* `forgematrix_v4` for CMFD.
* **Windows**: `start-cmfd.bat` (WALLET/POOL/WORKER/DEVICES/MODE/clock locks/EXTRA; default pool Aria as in FreeForgeMiner),
  `start.ps1` validates CMFD wallet/URL when the pool is `cmfd+tls://`, ignores AUTO_UV/UV_OFFSET there.

## 2026-10-21 — implementation and first tests (Claude, Opus 5.5)

* Host builds on Windows (MSVC) and Linux (Shahtyor); `cargo test --release`: 6/6 (new: `cmfd::tests::stats_line`, `pool_and_wallet`).
* Engine: `cmfd/fetch-engine.sh` with the local release tarball → SHA-256 ok; package `cmfd/cmfd-miner` and
  `cmfd-v4-replay` **byte-identical** (`cmp`) to FreeForgeMiner 1.2.3 release. Windows: `fetch-engine.ps1` with the 1.2.3 zip ok.
* Self-test `freeforgempear --algo cmfd --self-test` (= `cmfd-v4-replay --self-test`: small + DP4A differential, `replay_self_test=EXACT`):
  RTX 3070 (GPU1) ok, RTX 4070 Ti (GPU2) ok, Windows RTX 4070 Laptop ok.
* Live, own pool Kavatar (`cmfd+tls://192.168.1.58:29445`, LAN address of the pool, share = 7 leading zero bits), wallet 307df1d1…,
  wrappers `gpu1-test.sh`/`gpu2-test.sh`, internal timeouts, 5 min each, same session, auto batch, mode speed,
  **GPC offset 0 for the test** (set via NVML, restored after: GPU1 +200, GPU2 +345 → unit set +330 again):

| GPU | miner | FW/s avg (≥60 s) | last | W avg | FW/kWh | accepted | rejected | stale | INTEGRITY |
|---|---|---|---|---|---|---|---|---|---|
| RTX 4070 Ti (PL 190 W), batch 64 | FFGP 1.6.0 cmfd | 36.76 | 37.33 | 187.4 | 706 k | 84 | 0 | 4 | ok |
| | FreeForgeMiner 1.2.3 | 36.84 | 37.24 | 188.1 | 705 k | 81 | 0 | 17 | ok |
| RTX 3070 (PL 135 W), batch 32 | FFGP 1.6.0 cmfd | 14.11 | 14.43 | 134.8 | 377 k | 19 | 0 | 2 | ok |
| | FreeForgeMiner 1.2.3 | 14.20 | 14.55 | 134.8 | 379 k | 34 | 0 | 3 | ok |

  Parity within noise (−0.2 % / −0.6 %, same engine binary; the host adds no GPU work). 3070 is power-limited to 135 W on
  Shahtyor (FreeForgeMiner's 20 FW/s is at ~179 W). Stale = block changed in flight (harmless). Expected shares at 7 zero bits ≈ FW/128:
  4070 Ti FFGP ≈ 84 expected / 88 found (acc+stale), FFM ≈ 102 / 98; 3070 FFGP ≈ 30 / 21 (−1.7σ), FFM ≈ 37 / 37.
  The low 3070 FFGP count is within Poisson noise for one 5-min run, but should be re-checked on a longer run.
* **Found:** with the owner's Pearl offsets (+345 on 4070 Ti, +200 on 3070) left on the card, the CMFD engine fails at
  once — FFGP *and* FreeForgeMiner 1.2.3 alike ("illegal memory access" / "misaligned address"). Result: FFGP now prints a
  hint on such errors and gives up a GPU after 5 failed starts instead of reconnecting forever.
* Bench (`~/cfminer/bench/bench.py`, RUNBATCH with random coefficients) dies on 1.2.3 with an illegal memory access even
  on the reference binary — that old bench is not valid for 1.2.3; not used.
* Not tested yet: dev-fee slice (first slot 5–65 min after start, runs were 5 min), Windows model download/live mining,
  HiveOS package on a real rig, multi-GPU in one process, eco mode, `--per-gpu-workers`, long runs.

## 2026-10-21 (2) — towards rc 1.6.0 (Claude, Opus 5.5)

Owner decisions: default pool Aria, Midas Mining second (Kavatar and cmfd-pool.online in the examples); a Pearl undervolt is
reset by the host in CMFD mode; checks on Aria (Kavatar's connection is unstable); GPU2 (4070 Ti) not away from Pearl for long.

Done (commits e33d280, 260704b and the one with this entry):
* **Offset reset** (`host/src/cmfd_offset.rs`): a positive core V/F offset → 0 before the engine starts, put back on exit;
  crash marker `cmfd-offset-restore.json` undone at the next start; `--uv-offset N` forces N (user's risk); negative offsets
  untouched; no root → warning, mining goes on. Also used by `--self-test` when it runs the test vector.
* **Midas Mining**: URL from its API `https://midasmining.net/api/v1/pool` → `cmfd+tls://24.107.152.151:29745?pin=2ee4e6a0…e80e`
  (the landing page says :29445 — wrong); pin checked = SHA-256 of the DER certificate it serves (openssl, one handshake).
* **cmfd1 addresses** (Common Foundry wallet 1.0.16): `wallet_hex` decodes bech32m (BIP-350, HRP `cmfd`; `tcmfd` refused) to the
  hex key the pool/engine take; codec after `crates/cmfd-node/src/address.rs` 1.0.16; unit test with the upstream fixed
  vectors (`cmfd10xlxvl…4yhywr` = generator x), checksum/case/junk errors, round-trip of real keys.
* **Test vector**: `cmfd/fixtures/mainnet-template-nonce0-count32.coefficients.bin` from the CF 1.0.16 AMD HIP qualification
  (MIT, attribution in fixtures/README.md, NOTICE); `--self-test --algo cmfd` runs RUNBATCH 32 twice when the model is present,
  SHA-256 must be `fe67b80a…f4a7a7`.
* Windows: `PREPARE-V4-INPUTS.ps1` failed under a pwsh-7 parent (inherited PSModulePath → Get-FileHash missing) → removed for
  the child. `start-cmfd.bat`: pools, UV_OFFSET; `start.ps1`: cmfd1 wallets; README-WINDOWS.txt.
* MANUAL.md section 13 (EN/RU/ZH), README, HIVEOS.md, cmfd/README.md.

Checks (Shahtyor through gpu1/gpu2-test.sh; Clawa):
| check | result |
|---|---|
| unit tests | 7/7 (incl. `cmfd1_addresses`) |
| crash: `kill -9` of the host while mining (4070 Ti) | offset stayed 0 with the marker; no engine left behind; next start: "+345 put back", then 0 again |
| Midas Mining, 2 min, 4070 Ti | 26 accepted / 0 rejected / 2 stale, 36.7 FW/s |
| HiveOS package (build.sh tarball unpacked, h-config/h-run/h-stats with redirected paths), eco, 4 min, 4070 Ti | ok; h-stats `{"hs":[37.24],"hs_units":"hs","ar":[50,0],"algo":"forgematrix_v4"}`; 61/0/3 |
| long run 4070 Ti, Kavatar, stopped at 30 min (owner wanted the card back) | 36.94 FW/s @ 189.7 W, 443 acc / 0 rej / 58 stale, offset +345 put back |
| 3070 on **Aria**, 25 min | 14.34 FW/s @ 134.7 W (PL 135 W), 1 acc / 0 rej (Aria's share difficulty is high: ~1 share per 20k FW) |
| test vector on CUDA (FFM 1.2.3 replay, 3070, 3 runs) | `fe67b80a…f4a7a7` ×3, 2.18 s per batch of 32 |
| `--self-test --algo cmfd` with the model (3070) | EXACT + test vector 2/2 `fe67b80a…` (24 s with model load), offset +200 → 0 → +200 |
| Aria 10 min with a **cmfd1** wallet (3070) | key decoded to 307df1d1…05f8, payout correct, 14.32 FW/s, 1/0 |
| Windows: model download via the host (pwsh 7 parent) | 23.5 min first download, 15 s verify on the next start; engine stopped cleanly, no orphans |
| Windows package (build-windows.ps1) | zip with cmfd/ (engine, fixtures, licenses), start-cmfd.bat |

Not done / open: dev-fee slice not observed (first slot is 5–65 min after start; runs were ≤30 min); several GPUs in one
process not tested (both wrappers share one lock, nesting deadlocks — needs a two-GPU wrapper); live CMFD mining on Windows
(no 8 GB-free NVIDIA card here); a real HiveOS rig; release builds in the glibc-2.31 container; Aria share statistics are thin
(1 share per ~25 min on a 3070), longer runs for reject statistics need a pool with a lower share difficulty or hours.
