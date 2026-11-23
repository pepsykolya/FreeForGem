# CMFD (Common Foundry) — user notes

CMFD mining arrived in FreeForGemPear 1.6.0 and is part of FreeForGem. The full guide is
[MANUAL.md](../MANUAL.md), section 13 (NVIDIA) and section 14 (AMD); this page collects the setup facts, the pools and the
measured numbers from the CMFD development.

## How it works for you

* The CMFD work is done by the **FreeForgeMiner** engine (MIT, <https://github.com/pepsykolya/freeforgeminer>), shipped
  unchanged in `cmfd/` of the package and pinned by SHA-256. FreeForGem is the front end: one command line, clock locks,
  power in the stats line, the HiveOS package and the Windows launcher are the same as for Pearl. One engine runs per GPU.
* `--algo cmfd` is the default for a `cmfd+tls://` pool URL (`--algo pearl` with such a URL is an error).
* **Pool URL:** `cmfd+tls://IPv4:PORT?pin=<64 hex>` — TLS with the pool's pinned certificate.
* **Wallet:** the 64-hex payout key or a `cmfd1...` address of the Common Foundry wallet (testnet `tcmfd...` addresses are
  refused). Plus a worker name.
* **Model:** each hash ("FW") is one forward pass of the 6.4 GB ForgeMatrix V4 model. It is downloaded and prepared on the first
  start (Windows: about 23.5 min for the first download, 15 s to verify on the next start). Location: on HiveOS
  `/hive/miners/custom/cmfd-model` (shared with FreeForgeMiner: one download per rig), else `cmfd-model/` next to the binary.
  The engine's work folder is on the disk, not on the HiveOS flash.
* **Options:** `--cmfd-mode speed|eco`, `--cmfd-batch N`, `--cmfd-model DIR`, `--cmfd-dir DIR`, `--per-gpu-workers`,
  `--cmfd-skip-prepare`; environment `FFGP_CMFD_DIR`, `FFGP_CMFD_MODEL`, `FFGP_CMFD_WORK`.
* **Stats line** every 10 s, for example
  `[ 290s] GPU0 37.04 (190 W, 701390 FW/kWh) | total 37.04 FW/s | … | user sub 84 acc 80 rej 0 | stale 4 | height 25089 [DEVFEE]`.
  The mining start line names the backend and kernel of each GPU. *Stale* = the block changed while the share was in flight
  (harmless).
* **Self-test:** `--algo cmfd --self-test` runs the engine's self-test and, when the model is present, the official Common
  Foundry test vector.
* An engine that exits is restarted with growing pauses; after 5 failed starts in a row the GPU is left alone (pools ban
  reconnect storms); when every GPU has failed the miner exits with an error. Ctrl-C stops the engines gracefully (up to 10 s);
  a second Ctrl-C stops them at once.
* **Dev fee:** CMFD uses only the engine's built-in 1 % (36 s per hour per GPU, random slot, same pool, worker `ffm-fee`); the
  Pearl dev fee is not started in CMFD mode, so the total stays 1 %.

## HiveOS

* The coin follows the flight sheet's pool URL (a `?` in the URL is fine). Set *Hash algorithm* `forgematrix_v4`; the hashrate
  is shown in FW/s, the same as FreeForgeMiner.
* The Linux engine is built for Ubuntu 20.04 (glibc 2.30+), so it runs on every HiveOS image. The miner says clearly when the
  system is too old instead of restarting in a loop.
* The model folder must be writable: on a read-only folder the engine waits forever with "Another miner is preparing the
  shared model" (the HiveOS folders are writable).

## Windows

`start-cmfd.bat` (in the package): `WALLET`, `POOL`, `WORKER`, `DEVICES`, `MODE`, clock locks, `UV_OFFSET`, `AUTO_UV`,
`UV_MAX` / `UV_MIN` (NVIDIA), `UV_GOAL` (AMD), `EXTRA`. The default pool is Aria, as in FreeForgeMiner; the other pool URLs are in
the file. `start.ps1` checks the CMFD wallet and URL when the pool is `cmfd+tls://`.

## Pools

* **Aria** — the default. Its share difficulty is high: about 1 share per 20 000 FW, i.e. about one share per 25 min on an
  RTX 3070 (14 FW/s); short runs often show no share at all.
* **Midas Mining** — `cmfd+tls://24.107.152.151:29745?pin=2ee4e6a0…e80e` (the full pin is in `start-cmfd.bat`; the port on the
  pool's landing page, 29445, is wrong). RTX 4070 Ti, 2 min: 26 accepted / 0 rejected / 2 stale, 36.7 FW/s.
* cmfd-pool.online is used in the examples.

## Undervolt and clocks

* **A Pearl undervolt breaks CMFD.** With a Pearl offset left on the card (+345 on an RTX 4070 Ti, +200 on an RTX 3070) the
  engine fails at once ("illegal memory access" / "misaligned address") — FreeForgeMiner alone does the same. So in CMFD mode
  a positive core V/F offset is set to 0 before the engines start and put back on exit (also after a crash, at the next start);
  `--uv-offset N` forces N (your risk); negative offsets are left alone. The log gives a hint on such errors.
* **Memory clock:** a memory clock lock far below the card's maximum makes CMFD much slower (a Pearl OC with memory lock 810 MHz:
  RTX 3070 3.7 FW/s instead of 21.8 FW/s). The miner warns once per GPU when the memory clock is below half of the board maximum;
  since 1.7.6 the memory lock of the HiveOS profile is not applied for CMFD.
* **`--auto-uv` for CMFD on NVIDIA** (since 1.6.3 / 1.7.1-beta): a CMFD offset of its own per card, every step checked
  bit-exactly on the real model plus the official test vector, saved in `uv.json` (section `cmfd`). Later starts use a verified
  result directly (within `--uv-reval-hours`) or after a quick check; a failure falls back to the base offset, paused for 24 h.
  A changed power limit (more than 3 %) tunes again, because the edge moves with the operating point (RTX 3070: edge +225 at
  PL 135 W, +165 at PL 220 W). While mining, an `INTEGRITY | ERROR` or a GPU fault on a tuned card steps it back and restarts
  its engine.
* **`--auto-uv` for CMFD on AMD Radeon** (Windows): goals `UV_GOAL` = `eff` / `bal` (default: the most FW/W with at least 90 %
  of the stock FW/s) / `max`; your own settings are put back on exit.

## Measured

NVIDIA, short pool runs, mode speed, offset 0:

| GPU | FW/s | W | FW/kWh |
|---|---|---|---|
| RTX 4070 Ti (PL 190 W), batch 64 | 36.76 | 187.4 | 706 k |
| RTX 3070 (PL 135 W), batch 32 | 14.11 | 134.8 | 377 k |

RTX 4070 Ti, 30 min: 36.94 FW/s at 189.7 W, 443 accepted / 0 rejected / 58 stale. An uncapped RTX 3070 does about 20 FW/s at
~179 W.

`--auto-uv` on NVIDIA (Aria, averages of the stats line after the model load):

| card / setting | stock (0) | tuned offset | tuned | FW/kWh |
|---|---|---|---|---|
| RTX 3070, PL 135 W | 13.84 FW/s, 134.8 W, 369.6 k | +195 | 13.97 FW/s, 134.9 W, 372.8 k | +0.8 % (power-bound, voltage at the floor) |
| RTX 3070, PL 220 W | 19.51 FW/s, 214.2 W, 327.9 k | +135 | 20.41 FW/s, 212.9 W, 345.1 k | **+5.2 %** |
| CMP 50HX, PL 225 W, lock 1410 | 13.10 FW/s, 215.6 W, 218.7 k | +225 (no edge up to the limit) | 13.21 FW/s, 195.6 W, 243.2 k | **+11.2 %** |
| CMP 50HX (second card) | 13.01 FW/s, 217.0 W, 215.8 k | +225 | 13.17 FW/s, 203.2 W, 233.3 k | **+8.1 %** |

Tuning time: RTX 3070 about 16 min, 4x CMP 50HX in parallel about 17 min. RTX 3070 at PL 135 W, 20 min on Aria at +195:
14.12 FW/s, 134.3 W, 378.5 k FW/kWh, 1 accepted / 0 rejected.

`--auto-uv` on AMD, RX 6800 XT:

| setting | clock / voltage | FW/s | W | FW/kJ |
|---|---|---|---|---|
| stock | 2324 MHz / 1150 mV | 10.1 | 280 | 36 |
| `eff` = `bal` | 2100 MHz / 881 mV | 9.13 | 193 | 47.2 |
| `max` | 2324 MHz / 1075 mV | 10.07 | 248 | 40.6 |

On the pool: `bal` 15 min 9.06 FW/s at 191 W, `max` 10 min 10.03 FW/s at 247 W, stock 10 min 9.86 FW/s at 277 W; 0 rejected.
CMFD speed follows the core clock; a memory overclock does not help (and 2150 MHz memory gives wrong results).
