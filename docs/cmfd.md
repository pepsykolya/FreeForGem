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
* Live, own test pool (LAN, share = 7 leading zero bits), wallet 307df1d1…,
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

Owner decisions: default pool Aria, Midas Mining second (cmfd-pool.online in the examples); a Pearl undervolt is
reset by the host in CMFD mode; checks on Aria; GPU2 (4070 Ti) not away from Pearl for long.

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
| long run 4070 Ti, own test pool, stopped at 30 min (owner wanted the card back) | 36.94 FW/s @ 189.7 W, 443 acc / 0 rej / 58 stale, offset +345 put back |
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

## 1.6.0 release integration (21.10, branch `rel-1.6.0`)

Base `cmfd-amd` 6875658 (= `cmfd` bdb6d6d + `amd-uv-host` + AMD CMFD), plus: rc3 RAM gate for `--auto-uv` (4182700),
engine pin FreeForgeMiner **1.2.4** (cmfd1 addresses, random nonce start), AMD prebuilts from pearl-miner `rel-1.6.0`
(amd 665e5a8 saddr kernel + cmfd-amd 114946c ADLX power/UUID + amd-uv 6bedabf amduv.exe), AMD host feeding fix (eda9785).

Found and fixed during QA:
* `amduv.exe` was not packed into the Windows zip (missing copy; then a `\a` escape in the edit) — packed now.
* Windows start files failed on AMD-only PCs (`nvidia-smi.exe not found`) — AMD cards are detected and listed now.
* AMD CMFD mining used the CUDA worker path (`no CUDA-capable device`) — fixed in 6875658 (cmfd-amd agent), re-checked here.
* Linux CMFD engine (FreeForgeMiner release, built on Ubuntu 22.04) needs glibc 2.34: on Ubuntu 20.04 HiveOS images CMFD
  could not start (`GLIBC_2.32 not found`). `cmfd/build-engine-compat.sh` builds cmfd-miner / cmfd-v4-replay / cmfd-launch from
  the same pinned source tree (ec8dbc17… = FreeForgeMiner 1.2.4) inside ubuntu:20.04 (nvcc 12.9, same flags): need glibc 2.30
  (replay 2.17). build.sh ships them in the Linux/HiveOS packages and writes their SHA-256 into `cmfd/ENGINE`; the host checks
  the glibc need from that file and says so clearly instead of a restart loop.
* Mining start line per GPU now names backend and kernel (`GPU 0: … (cuda backend, kernel t256 t256s-F2c dcc8446f)`).
* Minor, not fixed (engine side): a read-only model folder makes `production-v4-inputs.py` wait forever ("Another miner is
  preparing the shared model"); HiveOS folders are writable.

## 1.6.1 hotfix (21.10, branch `rel-1.6.1` from v1.6.0)

Field bug (Rig02, HiveOS, 4x RTX 3070, 1.5.4rc2 and 1.6.0): `miner stop` sends SIGINT to the whole `freeforgempear | tee`
pipeline; tee exited first, the miner put back GPU0's offset, then `println!` panicked on EPIPE (Rust ignores SIGPIPE, so a
dead pipe is an error, and std's println! panics on it) and GPU1-3 stayed undervolted with locked clocks until the next start.
On Windows the same panic happens as soon as the PowerShell pipeline of start.ps1 stops reading (Ctrl+C).
* println!/print!/eprintln! shadowed crate-wide (main.rs) by macros that drop write errors.
* Shutdown order: Pearl restores uv offsets (all cards in parallel) and clocks right after the main loop, then joins workers
  and logs; CMFD stops all engines together, then restores the offset and clocks, then logs. Restore functions log only after
  every card is set. `step_back` never sets an offset after the card was handed back.
* Windows console close / logoff / shutdown: own console handler restores synchronously (ctrlc's handler returns at once and
  Windows ends the process right after it, so nothing was restored before).
* h-run.sh: `tee -i`.
* CMFD: once per GPU after 30 s of mining, memory clock < 1/2 of the board maximum -> WARNING (Pearl OC with Mem lock 810 cuts
  CMFD ~6x: Rig02 3070 3.7 FW/s locked vs 21.8 FW/s at ~237 W free; Shahtyor 3070 3.6-3.7 FW/s with -lmc 810).
* Pearl `--self-test` without `--devices` checks every GPU.

Checks (Shahtyor GPU1 3070 + GPU2 4070 Ti in one miner under gpu1-test.sh, GPU2 units paused under the same lock; owner's
offsets +200/+300; SIGINT to the process group of `miner | tee`; note: a job started with `&` from a script has SIGINT
ignored, which tee inherits, so the launcher must reset SIGINT to default or tee survives and nothing reproduces):
* 1.6.0, tee dies: offsets +200/+215 (GPU2 left), clocks still locked, uv-restore.json + clocks-restore.json left = the bug.
* 1.6.1, tee dies: +200/+300, clocks reset, no state left. tee -i, 3 min on the pool: 14/14 accepted, restore lines in the log.
* 1.6.1 CMFD, tee dies, GPU1 memory locked 810: WARNING on GPU1 only; offsets 0 -> +200/+300 back, marker removed.
* Self-tests without --devices: Pearl GPUs [0, 1] PASSED; CMFD both ok (official 1.0.16 vector). Clawa 4070 Laptop: Pearl
  self-test PASSED; Windows reader-gone test: 1.6.0 exits 101 (panic) at once, 1.6.1 mines on and ends normally (exit 0).
* cargo test --release: 7/7. Packages: libpearlgpu.so, pearlgpu.dll, pearlgpu_amd.dll, amduv.exe, cmfd/* byte-identical to 1.6.0.
* Not tested on hardware: the Windows console-close restore (needs an Administrator session with an offset/clock lock set).

## 1.6.2 (branch `rel-1.6.2`): per-card undervolt limits

* `--uv-max`, `--uv-min`, `--uv-offset` (and `UV_MAX` / `UV_MIN` / `UV_OFFSET` in the .bat files): one value for all GPUs or a
  comma list with exactly one value per mining GPU in `--devices` order (`-` = that GPU's default); a wrong length is an error.
  CMFD uses the `--uv-offset` list the same way.
* Without `--uv-max`: CMP 30HX/40HX +150; CMP 50HX (TU102: PCI id 0x1e09 or "50HX" in the name) and Turing RTX 20xx /
  GTX 16xx +225 (owner's CMP 50HX rigs hold +220 and more); others +495 (compute capability 7.5 decides; the name when NVML
  gives none). CMP 50HX mines Pearl at only ~2.6 TH/s per card (int8 tensor cores cut ~17x against the 40HX; same on drivers
  550.78 and 595.91.07, 3x CMP 50HX rig): not recommended for Pearl (GPU tables in README/MANUAL/HIVEOS). Search start: RTX 50 (sm_120) at least +300, RTX 40 at least +200 (a default start
  at or above a lower `--uv-max` is dropped). An explicit `--uv-max` above the card default is used, with a WARNING.
* Start log: `auto-uv: GPUn (bus .., NAME): range +A..+B (max: <source>; start: <source>)`.
* uv.json gets `cap` (the limit of the tuning). Saved offset above the current limit: run at the limit after a quick check, the
  saved result stays; saved offset = old limit and the limit is higher now: tuned again. Entries without `cap` (1.6.1) are never
  re-tuned for it, but a lower limit is respected (1.6.1 applied a saved offset above a lowered `--uv-max` as is).
* "restored twice" on Rig02: not a duplicate. The 1.6.0 run before the 1.6.1 update ended without restore lines (the 1.6.0 bug)
  and left GPU1-3 at +270/+270/+315; the 1.6.1 start put them back from uv-restore.json. Recovery lines are now worded
  differently (`(bus 04, RTX 3070) offset +270 -> +0 put back (left changed by the previous run)`, unchanged cards: `already at`).

Checks (Shahtyor GPU1 3070 + GPU2 4070 Ti in one process under gpu1-test.sh, GPU2 units paused under the same lock for 318 s;
own state dir, offsets set to 0 for the test, owner's +200/+300 and the units back at the end, GPU0 untouched):
* cargo test --release: 14/14 (new: list parsing, lengths, validation, defaults by name/cc incl. the 2060S/3070L/CMP rig,
  per-card ranges and warnings, saved result vs limit, uv.json `cap`).
* dry run, no `--uv-max`: 3070 `+120..+495 (max: default)`, 4070 Ti `+200..+495 (start: RTX 40 default)`.
* `--uv-max 150,330`: ranges +120..+150 / +200..+330; 3 values for 2 GPUs and `--uv-min 200,-` above 150 are refused at start.
* short tune (`--uv-step 30`): 3070 stopped at +150 (cap), 4070 Ti +200 -> +320 (next step +350 > 330); uv.json cap 150 / 330.
* `--uv-max 120,330`: 3070 "saved +150 is above the limit +120, limited to it", quick check ok, uv.json still 150; 4070 Ti +320.
* GPU1 alone `--uv-max 180`: "the limit was raised (+150 when tuned, +180 now) ... tuning again" -> +180, cap 180.
* Linux build in ffgp-build (glibc 2.30 max); Windows build; libpearlgpu.so, pearlgpu.dll, pearlgpu_amd.dll, amduv.exe, cmfd/*
  byte-identical to 1.6.1. Windows chain start.bat -> start.ps1 -> exe passes `UV_MAX=225,-,150` intact.
* Not tested on hardware: Turing / CMP defaults (no such card; unit tests only), RTX 50 start +300.

## Branch `cmfd-nv-uv` (from 1.6.2): `--auto-uv` for CMFD on NVIDIA

Engine unchanged (FreeForgeMiner 1.2.4 release / the 1.6.2 Ubuntu 20.04 rebuild, same SHA-256): its CUDA worker's `--server`
protocol already has what the check needs (`RUNBATCHDIGEST n coef prefix chal` -> `<prefix>-final-digest.bin`, `RUNBATCH 32` for
the official test vector), so the host drives it directly; no engine patch, nothing added to `ENGINE.lock` / `patches/`.

* `cmfd_uv.rs`: per card one `cmfd-v4-replay --server MODEL` (model loaded once per tuning; loads one card at a time), two fixed
  batches (splitmix64 bytes, mining batch size: 64 from 11 GB, else 32, or `--cmfd-batch`), digests folded with BLAKE3; every run
  must equal the first run of its batch and the reference taken at the base offset (0, or the owner's offset when it is 0 or
  below). Base 60 s + official test vector; steps `--uv-step` from `--uv-min` (default base + one step) to `--uv-max` / the 1.6.2
  per-card default (`card_defaults`), 30 s + 10 s measurement each; first failure = edge, one step back (`--uv-keep-last`),
  confirmation 120 s + test vector (one more step down on failure). NVML watts / clock / temperature sampled in the measurement.
  Xid / no answer in 240 s = driver fault (crash marker `cmfd_trying` kept, the next start tunes below it).
* `uv.json`: section `cmfd` (Pearl's `gpus` untouched), key = UUID; identity = driver + "engine | worker sha256[..12] | mode |
  batch | inputs version [| lock-cclock N]"; enforced power limit stored, a change > 3 % re-tunes (the edge moves with the
  operating point: RTX 3070 PL 135 W edge +225, PL 220 W edge +165). Later starts: verified within `--uv-reval-hours` -> applied,
  else 20 s + 8 s quick check + test vector; failure -> base, paused 24 h.
* Offsets only through `cmfd_offset` (`apply_managed` records every card before the first change; `set` / `current` / `orig`),
  so the existing restore (exit, second Ctrl-C, console close, crash marker at the next start) puts the owner's offset back.
* While mining: `INTEGRITY | ERROR` / CUDA fault of a tuned card -> one step back (saved; at the base: paused 24 h), the engine
  is restarted. Without a usable check (no model/worker, failure at the base) the card mines at the base with a WARNING.
* Fix found in the tests (also in 1.6.2): an engine stopped while its worker still loads the model left the worker running
  (4 orphaned `cmfd-v4-replay` on Rig05, 7 GB of GPU memory each). `stop_children` now collects the workers of every engine
  (Linux /proc) and ends those left after the stop.
* Windows: `start-cmfd.bat` `AUTO_UV`, `UV_MAX`, `UV_MIN`; `start.ps1` passes AUTO_UV for CMFD. AMD: unchanged (warning).

Measurements (Aria, averages of the stats line after the model load):

| card / setting | stock (0) | result | tuned | FW/kWh |
|---|---|---|---|---|
| RTX 3070, PL 135 W (Shahtyor GPU1) | 13.84 FW/s, 134.8 W, 369.6 k | edge +225 (digest), **+195** | 13.97 FW/s, 134.9 W, 372.8 k | +0.8 % (noise: power-bound at ~1050 MHz, voltage at the floor) |
| RTX 3070, PL 220 W (temporary) | 19.51 FW/s, 214.2 W, 327.9 k | edge +165 (rerun_diff), **+135** | 20.41 FW/s, 212.9 W, 345.1 k | **+5.2 %** |
| CMP 50HX GPU2, PL 225, lock 1410 (Rig05) | 13.10 FW/s, 215.6 W, 218.7 k | no edge up to the +225 limit, **+225** | 13.21 FW/s, 195.6 W, 243.2 k | **+11.2 %** |
| CMP 50HX GPU3 | 13.01 FW/s, 217.0 W, 215.8 k | **+225** | 13.17 FW/s, 203.2 W, 233.3 k | **+8.1 %** |
| CMP 50HX GPU0 / GPU1 (worker numbers, tuning only) | 13.11 / 222.4 W; 13.07 / 219.9 W | +225 | 13.39 / 221.2 W; 11.70 / 194.9 W (GPU1 80 °C, throttling) | +2.6 % / +1.0 % |

Tuning time: RTX 3070 ~16 min (14 steps), 4x CMP 50HX in parallel ~17 min (15 steps). The references are the same on every card
and power limit (they are pure math): `0870ca17...`, `c6d160e6...` for batch 32.

Final package (this commit) on Shahtyor GPU1: the integrity watchdog with a known-bad +225 at PL 220 W stepped +225 -> +210 ->
+195 -> +180 (each `INTEGRITY | ERROR`, engine restarted after 5 s, never DISABLED); back at the owner's PL 135 W the result was
re-tuned (power limit changed): +240 failed, the confirmation at +210 failed too, +195 confirmed + test vector; 20 min on Aria
at +195: 14.12 FW/s, 134.3 W, 378.5 k FW/kWh, 1 accepted / 0 rejected. Owner's +175 back after every run; Rig05 owner's +150
back on all cards, `miner start` (with the HiveOS PATH) verified. Pearl self-test PASSED (lines differ from 1.6.2 only by the
random nonce_base of each run; Pearl code unchanged).

## amd-cmfd-uv (branch from `rel-1.6.2`): undervolt for CMFD on AMD (`--auto-uv`, `AUTO_UV=1` in start-cmfd.bat)

Measured on the owner's RX 6800 XT (Adrenalin 25.5.1, ASIC watts from ADLX; kernel bench 2–2.5 min per point + 45–60 s
bit-exact check on the real model + CF 1.0.16 test vector):

* Stock (2324 MHz max, 1150 mV): 10.1 FW/s, 280 W, 36 FW/kJ. FW/s follows the core clock; the memory runs at full clock all the
  time (unlike Pearl), +7.5 % memory clock and fast timings change nothing (and 2150 MHz memory gives wrong results). PL only
  down to −6 % in ADLX: 9.9 FW/s, 264 W (+4 % FW/W).
* Voltage floor 881 mV: 1900–2200 MHz max pass (8.4–9.6 FW/s at 182–201 W, 46–48 FW/kJ, +28..33 %); 2250 and 2300 fail at
  every slider value up to 1000–1025 mV. 2200/881 passed twice and failed once: the edge moves, one step of margin is needed.
* Owner's clock 2324 with the slider at 1100/1075 mV: 10.2 FW/s at 258/246 W (+9/+15 %); 1050 failed once, passed once; 1000 fails.
* The official test vector (32 nonces × 2) alone is not enough to judge a point: it passed at 2324/1050 and at memory 2150 where
  the long check found wrong digests.

Implementation: pearl-miner `amd-cmfd-uv` — the OpenCL worker got `--uvcheck MODEL secs measure [refs]` (two fixed batches of
32 nonces, final digests must equal the stock ones bit for bit, then FW/s), amduv got `--algo cmfd --model BANK` (own result
section per card, search A = floor voltage / clock up → `eff`, search B = owner's clock / voltage down → `max`, `bal` = most FW/W
with ≥ 90 % of stock FW/s, each confirmed by a long check; `apply --goal`). Host: `cmfd.rs` starts `amduv apply --algo cmfd`
on AMD with `--auto-uv` after the model is prepared (guard restores the owner's settings on exit), goal `$FFGP_AMD_UV_GOAL`
(default `bal`); NVIDIA unchanged (`--auto-uv` ignored for CMFD with a warning). start-cmfd.bat: `AUTO_UV`, `UV_GOAL`.

Checks (owner PC, test package = 1.6.2 + new freeforgempear.exe / amduv.exe / cmfd-v4-replay-ocl.exe):
* `amduv tune --algo cmfd`: 16 min; eff = bal = 2100/881 (9.13 FW/s, 193 W, 47.2 FW/kJ), max = 2324/1075 (10.07 FW/s, 248 W,
  40.6 FW/kJ); owner's settings read back unchanged afterwards.
* Aria pool through the host with `--auto-uv`: bal 15 min 9.06 FW/s / 191 W (47.4 FW/kJ), max 10 min 10.03 FW/s / 247 W
  (40.7), stock 10 min 9.86 FW/s / 277 W (35.6); 0 rejected, INTEGRITY ok, the guard restored the owner's settings each time
  (read back: 500/2324/1150/PL 0/VRAM 2000/timing 0). Shares on Aria are rare (none in these 35 min).
* Pearl with the new amduv: stock 34.25 TMAC/s, 2200/881 32.2 at 173 W (as before). NVIDIA (Clawa): Pearl self-test PASSED,
  `--algo cmfd --auto-uv` prints the NVIDIA warning as before. cargo test --release 14/14.
* Prebuilt: amduv.exe and cmfd/cmfd-v4-replay-ocl.exe replaced (the binaries tested above); pearlgpu.dll/.lib,
  pearlgpu_amd.dll, cmfd-miner-ffgp.exe, libpearlgpu.so byte-identical to rel-1.6.2.

## 1.6.3 (bug-fix release): branches cmfd-nv-uv + amd-cmfd-uv, lock restore, auto-uv after an OC apply

* Merge of `amd-cmfd-uv` (fcc9f86) into `cmfd-nv-uv`: six text conflicts (both branches edited the same `--auto-uv` spots);
  resolved keeping both: NVIDIA -> `cmfd_uv`, AMD -> `amd_uv::start(.., Load::Cmfd)`; start-cmfd.bat has `UV_MAX/UV_MIN`
  (NVIDIA) and `UV_GOAL` (AMD). The AMD part was not re-run on AMD hardware for 1.6.3; its binaries are the ones tested above.
* `--lock-cclock/--lock-mclock`: the lock from before is put back (new `hiveoc.rs`: HiveOS OC profile, nvidia-oc rules of hive
  0.6-231; or `$FFGP_RESTORE_CCLOCK/MCLOCK`); recorded in `clocks-restore.json` for the crash path. NVML has no getter for a
  lock (checked on driver 580.178: nvidia-smi -q, GetPerformanceModes, GetCurrentClockFreqs, MinMaxClockOfPState).
* `--auto-uv` (Pearl): a read-back of 0 / the HiveOS profile value / the owner's starting offset = OC apply -> set the verified
  offset again + quick check (max 4/h); a check whose offset was reset by an OC apply is repeated; other values: stand down.
* Fixes found in the tests (also in 1.6.2): a stop during a check (HiveOS `miner stop` = Ctrl-C then SIGHUP) left the crash
  marker (`trying` / `cmfd_trying`) -> the next start treated the offset as a crash point; a stop during the start quick check
  paused auto-uv for 24 h. A re-check while mining takes its RAM slot before pausing the GPU.
* Not ported to CMFD: re-apply after an OC apply while mining (the CMFD tuner sets its offset before the engine starts; an OC
  apply mid-run leaves the card at the HiveOS value, which is safe).

Checks (Shahtyor GPU1 RTX 3070 through gpu1-test.sh; fake HiveOS profile via `NVIDIA_OC_CONF`):

| test | result |
|---|---|
| prior lock 1200/810, run `--lock-cclock 1500 --lock-mclock 5001`, Ctrl-C / SIGTERM | 1500/5001 while running; after exit under load 1200/810 (both) |
| same, SIGKILL, then a plain start | marker `core_prev 1200, mem_prev 810`; the next start put 1200/810 back |
| no profile, `FFGP_RESTORE_CCLOCK=1250`, SIGTERM | 1260/810 after exit |
| no profile, no env | reset (1620/6801 under load, as before 1.6.3) |
| auto-uv, mining at +210: offset -> 0, -> 30 (profile), -> 45 (by hand) | re-applied (+210 failed the quick check -> +195 saved, ok), re-applied +195, stood down |
| OC apply during the start quick check | check repeated at +195, passed |
| 2x SIGINT during a check (Pearl, CMFD) | no crash marker left, no 24 h pause (after 703f4d6), no CMFD worker left |
| Pearl 5 min, LuckyPool, lock 1500/810, +195 | 67.7 TH/s, 99 W, 0.685 TH/W, 13/13 accepted (stock offset: 67.67 TH/s, 123 W, 16/16) |
| CMFD 5 min, Aria, `--auto-uv` (+195 saved) | 14.2-14.4 FW/s, 135 W, ~380 k FW/kWh, 1/0 shares; owner's +175 back |
