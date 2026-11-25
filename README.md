# FreeForGem

**FreeForGem** is a multi-coin GPU miner for **Pearl (PRL)**, **Common Foundry (CMFD)**, **NOID (Parano1d)**, **Proxima (PROX)** and,
since 1.8.0, **NEB** and **Ting / Daler (DLR)**, with its own GPU kernels, a CPU check of every share before it is submitted, a
**1% developer fee**, an optional **per-card automatic undervolt** (`--auto-uv`) that finds the lowest safe core voltage of each
GPU with bit-exact checks, and, since 1.8.0, **different coins on different cards** of one rig (`--split`).

FreeForGem is the multi-coin line of [FreeForGemPear](https://github.com/pepsykolya/FreeForGemPear) (since 1.7.0: binary
`freeforgem`, HiveOS custom miner `freeforgem`). FreeForGemPear continues as the Pearl miner in its own repository; saved
`--auto-uv` results of FreeForGemPear are imported automatically on the first start.

* **Download:** [Releases](https://github.com/pepsykolya/FreeForGem/releases) (latest: **1.8.1**)
* **Manual** (English / Русский / 中文): **[MANUAL.md](MANUAL.md)** — HiveOS, Linux, Windows, pools, overclocking, undervolt, troubleshooting
* **HiveOS step by step:** [HIVEOS.md](HIVEOS.md)
* **History / older versions:** [FreeForGemPear](https://github.com/pepsykolya/FreeForGemPear),
  [FreeForGemPear releases](https://github.com/pepsykolya/FreeForGemPear/releases)

## What's new in 1.8.1

* **auto-uv while mining on every coin:** cards keep mining and submitting shares while they are tuned; all cards at once as RAM
  allows; no built-in offset ceiling; much faster tuning (4x RTX 3070: NOID ~9 min instead of ~25, PROX ~5 instead of ~16).
* **Turing (RTX 20xx, GTX 16xx, CMP HX):** every result is confirmed 5 minutes on its mining load; repeated crashes go straight to
  the stable offset.
* **A table screen** with every GPU (hashrate, power, per watt, clocks, offset and tuning state, shares) and the latest events.
* **HiveOS:** `--hive-oc-sync` writes the tuned offsets and clock locks into the worker's OC profile; the miner window restarts
  itself; `--split` works from a flight sheet of any coin.
* **Pools:** a spoiled pool session is replaced automatically. **NEB:** engine nebgpu 0.8.4, faster on Turing.
* Full list, SHA-256 of the packages (English / Русский / 中文): [docs/release-1.8.1.md](docs/release-1.8.1.md).

## What's new in 1.8.0

* **NEB** (nebhash v2, MA/s): Suprnova pool, solo through your node or a gate for several rigs; engine nebgpu 0.8.1 with one
  engine process per GPU; full memory clock by default, `--table-mclock 810` for the table path, `--tbig auto`.
  [MANUAL.md](MANUAL.md), section 24.
* **Ting / Daler (DLR)** (GH/s): crypto-connection.net, your ffgt-gate or solo on a node; one engine process per GPU. Section 26.
* **Different coins on different cards:** `--split neb:0,1 pearl:rest` with coin-tagged values (`--pool neb=URL --wallet
  neb=ADDR --lock-cclock pearl=1410`); one miner process per group, each with its own pool, dev fee, clocks and restarts; works
  from one HiveOS flight sheet. Section 27.
* **Per-GPU lists of any length** in every coin: a shorter list gives its last value to the other cards, `-` skips a card
  (`--table-mclock 810,810,-,-` on 8 cards). Section 28.
* **Faults:** auto-uv resumes at the crashed offset minus 2 steps; after a GPU fault the card is put into a safe state before its
  engine starts again; FAULTY (a 10-minute pause) instead of DEAD for a card that still
  opens; opt-in `--reboot-on-lost-gpu MIN`; fewer stale shares and a warning when the rig's network answers late. Section 29.
* **Every coin:** yellow / orange auto-uv and auto-lock lines, the core lock checked under load (`[LOCK NOT HELD]`), auto-uv
  climbs while each step lowers the power, dev-fee slots retried, a memory clock policy per coin. Section 25.
* **New options:** `--split`, `--table-mclock`, `--tbig` / `--tbig-gb`, `--power-limit` / `--pl` (optional, never on by default:
  the miner limits the power with the core lock and the offset), `--reboot-on-lost-gpu`, `--debug-timing`.
* **CMFD:** engine FreeForgeMiner 1.2.5 (downloadable from its release page; it also knows the compact mainnet model).

## Supported coins

| Coin | Algorithm (HiveOS hash algorithm) | GPUs | Select with |
|---|---|---|---|
| **Pearl (PRL)** | pearlhash (`pearlhash`), proof of useful work, int8 tensor-core GEMM | NVIDIA RTX 20/30/40/50, GTX 16, CMP; AMD Radeon RX 6000+ (Windows) | default |
| **Common Foundry (CMFD)** | ForgeMatrix V4 (`forgematrix_v4`) | NVIDIA RTX 20xx or newer, AMD Radeon (Windows); ~7 GB free GPU memory | a `cmfd+tls://` pool URL or `--algo cmfd` |
| **NOID (Parano1d)** | Poseidon2b over GF(2^128) (`parano1d`), pool protocol `parano1d-stratum-v1` over TLS | NVIDIA sm_80+ (RTX 30/40/50, A100/H100), driver **580+** | `--algo noid` (or a NOID pool URL) |
| **Proxima (PROX)** | BLAKE2b-256 over the transaction hash (HiveOS: leave `----`, there is no `proxima` in the list) | NVIDIA GTX 10 / RTX 20/30/40/50 (sm_61+) | `--algo prox` (or a pool host with `prox`, an `innov+tls://` URL, `--solo`) |
| **NEB** (1.8.0) | nebhash v2 (`nebhash`), MA/s | NVIDIA GTX 10 / RTX 20/30/40/50, CMP | `--algo neb` (or a `neb.suprnova.cc` pool URL, `--node`, `--gate`) |
| **Ting / Daler (DLR)** (1.8.0) | Ting SHA-256 (HiveOS: leave `----`), GH/s | NVIDIA GTX 10 and newer (CUDA), AMD (OpenCL) | `--algo ting` (or a `crypto-connection.net` pool, a `ting://` gate URL) |

Different coins on different cards of one rig: `--split` (below and [MANUAL.md](MANUAL.md), section 27).

## Quick start

### HiveOS

Create a **new flight sheet**: Miner **Custom**, then *Setup Miner Config*:

| Field | Pearl | NOID | NEB | Ting / DLR |
|---|---|---|---|---|
| Miner name | `freeforgem` | `freeforgem` | `freeforgem` | `freeforgem` |
| Installation URL | `https://github.com/pepsykolya/FreeForGem/releases/download/v1.8.0/freeforgem-1.8.0_hiveos.tar.gz` | same | same | same |
| Hash algorithm | `pearlhash` | `parano1d` | `nebhash` | leave `----` |
| Wallet and worker template | `%WAL%.%WORKER_NAME%` | `%WAL%.%WORKER_NAME%` | `%WAL%.%WORKER_NAME%` | `%WAL%.%WORKER_NAME%` |
| Pool URL | `pearl-eu2.luckypool.io:3360` | `stratum+ssl://eu2.innovlab.cc:19601` | `stratum+tcp://neb.suprnova.cc:7804` | `crypto-connection.net:4073` |
| Pass | empty | empty | `x` | `x` |
| Extra config arguments | optional, e.g. `--auto-uv` | `--algo noid --noid-mode eff --lock-cclock 1300 --auto-uv` | e.g. `--auto-uv --lock-cclock 1800 --table-mclock 810` | e.g. `--lock-cclock 1800 --auto-uv` |

Use the **HiveOS** file of the release (`freeforgem-1.8.0_hiveos.tar.gz`) as the Installation URL — not the Linux or Windows archive.
Two coins on one rig from one flight sheet (main coin in the flight sheet, the other in Extra config): [HIVEOS.md](HIVEOS.md).
CMFD, NOCK merge mining, overclocking and undervolt on HiveOS: [HIVEOS.md](HIVEOS.md).

**CMP cards (optional, off by default):** under the stock NVIDIA driver they compute about 10 times slower. Adding `cmp-unlock-driver` to *Extra config arguments* (or `sudo ./freeforgem --cmp-driver`) makes the miner install an unlocked driver and **reboot the rig**; it replaces the kernel driver of the whole rig, read MANUAL.md section 20 first.

### Windows 10/11 x64

Download `freeforgem-1.8.0-windows-x64.zip`, unpack it to a folder without spaces, open the start file for your coin and
pool (`start-luckypool-eu/ru/us/asia.bat`, `start-kryptex-eu/ru/us/asia.bat` for Pearl, `start-cmfd.bat`, `start-noid.bat`,
`start-prox.bat`, `start-neb.bat`, `start-ting.bat`),
replace the example `WALLET` (the developer's address) with your own and run it (as Administrator for `--auto-uv` / clock locks).
Only the NVIDIA driver is needed (AMD Radeon: the AMD Adrenalin driver). Details: [MANUAL.md](MANUAL.md#5-windows-1011),
`README-WINDOWS.txt`.

### Linux x86_64

```
tar xzf freeforgem-1.8.0-linux-x86_64.tar.gz
cd freeforgem-1.8.0
./freeforgem --self-test                                   # optional: GPU kernel vs CPU reference
./freeforgem --pool pearl-eu2.luckypool.io:3360 --wallet YOUR_PRL_ADDRESS --worker rig1                      # Pearl
./freeforgem --algo noid --pool stratum+ssl://eu2.innovlab.cc:19601 --wallet o1YOUR_ADDRESS --worker rig1              --noid-mode eff --lock-cclock 1300 --lock-mclock 810 --auto-uv                                # NOID
./freeforgem --pool 'cmfd+tls://IP:PORT?pin=64HEX' --wallet cmfd1YOUR_ADDRESS --worker rig1                 # CMFD
./freeforgem --pool stratum+tcp://neb.suprnova.cc:7804 --wallet neb1YOUR_ADDRESS --worker rig1 --table-mclock 810  # NEB
./freeforgem --pool crypto-connection.net:4073 --wallet YOUR_TING_ACCOUNT --worker rig1                       # Ting / DLR
./freeforgem --split neb:0,1 pearl:rest --pool neb=stratum+tcp://neb.suprnova.cc:7804 --wallet neb=neb1YOUR_ADDRESS \
    --pool pearl=pearl-eu2.luckypool.io:3360 --wallet pearl=prl1YOUR_ADDRESS --worker rig1                    # two coins
```

The Linux binary is built on Ubuntu 20.04, so it runs on Ubuntu 20.04+ and HiveOS.

## Automatic undervolt (`--auto-uv`)

Optional, off by default. With `--auto-uv` the miner finds, **per card** (and for NOID per mode), the deepest core V/F offset at
which that GPU still computes correctly, applies it while mining and puts your own settings back on exit (needs
root/Administrator). Each GPU gets its own result, saved in `uv.json` together with the driver, kernel and power
limit, one profile per core clock lock (switching the lock reuses that lock's result, a new lock is tuned once while the other GPUs
mine); a crashing offset is stepped down by the miner itself. Typical gain: RTX 3070 on NOID, eff mode @1400 MHz: 87.5 W → 72.8 W at
the same hashrate (**+20 % MH/W**); Pearl @1560 MHz: roughly 5–10 W less per card. Mixed rigs: the limits are chosen per card
automatically. Details: [MANUAL.md](MANUAL.md), section 12 (Pearl), 13 (CMFD), 15 (NOID).

### The silent edge of undervolt

Near the stability limit a GPU does **not** crash and reports **no Xid**, the quick self-test passes — but some results are
already wrong (NOID, RTX 3070 @1560 MHz, +250: **168 of 5984** launches wrong). That means invalid or never-found shares that the
miner log does not show. FreeForGem's `--auto-uv` therefore checks every step bit-exactly over thousands of launches under full
load, keeps a safety margin below the first failure, and keeps checking results while mining (every hit is checked on the CPU):
a failure steps the offset back automatically. [MANUAL.md](MANUAL.md), section 16.

## Pearl (PRL)

Our own GPU kernels (closed source). Every share proof is built and checked with the official `zk-pow` verifier before it is
submitted.

RTX 3070 locked at 1560 MHz core / 810 MHz memory: **~70.5 TH/s at ~107 W**.

### Supported hardware

* **Tuned for:** RTX 30xx (Ampere, sm_86) — 3060 / 3060 Ti / 3070 / 3070 Ti / 3080 / 3090.
* **RTX 40xx (Ada, sm_89):** own kernel since 1.5.4 (RTX 4070 Ti @2445/810: 144.7 TH/s at ~169 W vs 144.3 at ~179 W with the RTX 30xx kernel). Tested on RTX 4070 Ti and RTX 4070 Laptop; other 40xx cards use the same kernel.
* **RTX 50xx (Blackwell, sm_120):** supported since 1.5.0 (not specially tuned yet), needs NVIDIA driver 570+.
* **Turing (RTX 20xx / GTX 16xx / CMP 30HX–50HX, sm_75):** supported since 1.5.3 with an own kernel (~49.9 TH/s on an RTX 2060 SUPER @1605 MHz); lock the memory at 5001, not 810. CMP 50HX gives ~67 TH/s per card at lock 1300 and ~77 TH/s at 1500 on driver 610.43.03 (166-176 W; 4x CMP 50HX rig; only ~2.6 TH/s on the older drivers 550.78 / 595.91.07) and mines CMFD well (~13 FW/s per card at ~220 W). On HiveOS, `cmp-unlock-driver` in Extra config installs the unlocked driver once (opt-in, reboots the rig; MANUAL.md section 20).
* **AMD Radeon (Windows only, since 1.6.0):** own kernel, tested on RX 6800 XT (RDNA2): 34.4 TH/s at ~260 W stock,
  ~30–32 TH/s at 160–172 W with `--auto-uv`. RX 6000 (RDNA2) supported; RX 7000/9000 should work but are untested; integrated Radeon
  graphics are skipped. No AMD build for Linux/HiveOS yet. The miner picks CUDA or AMD by itself (`--backend auto|cuda|amd`).
  Details: [MANUAL.md](MANUAL.md) section 14.
* **NOID:** NVIDIA sm_80+ only (RTX 30/40/50, A100/H100) with driver 580+; no Turing, no AMD.
* **Not supported:** older GPUs (Pascal and earlier), A100 (sm_80) for Pearl, AMD RX 5000 and older, AMD on Linux/HiveOS.
* **System:** Linux x86_64 or Windows 10/11 x64, NVIDIA driver 525 or newer (RTX 50xx: 570 or newer). The Linux binary is built on
  Ubuntu 20.04 (glibc 2.31), so it runs on Ubuntu 20.04+ and HiveOS.

Full manual in English, Russian and Chinese: **[MANUAL.md](MANUAL.md)** (HiveOS, Linux, Windows, pools, overclocking, troubleshooting).

### Options

| Option | Meaning |
|---|---|
| `--pool HOST:PORT` | stratum pool (default `pearl-eu2.luckypool.io:3360`) |
| `--wallet ADDRESS` | your PRL address (required) |
| `--worker NAME` | worker name (default `worker1`) |
| `--password TEXT` | stratum password (default `x`); on Kryptex `d=VALUE` sets a custom share difficulty |
| `--devices 0,1,...` | GPUs to use (default: all visible; honours `CUDA_VISIBLE_DEVICES`) |
| `--backend auto\|cuda\|amd` | GPU library: auto (default) = CUDA if it sees an NVIDIA GPU, else AMD (Windows) |
| `--minutes N` | stop after N minutes (default: run until Ctrl-C / SIGTERM) |
| `--self-test` | verify the kernel and exit |
| `--auto-uv` | optional (off by default): find, apply and supervise a safe undervolt of the GPU (V/F offset), needs root/Administrator, restores your settings on exit; see [MANUAL.md](MANUAL.md) section 12 |
| `--uv-offset N` / `--uv-reset` / `--uv-reset-all` | apply a fixed undervolt offset after a check / forget the saved profile of the current lock / forget all saved results |
| `--no-color` | no colours in the log (also `NO_COLOR=1`) |
| `--uv-max N` / `--uv-min N` | highest / starting undervolt offset; default per card (RTX 20xx / GTX 16xx up to +225, CMP 50HX up to +330, CMP 30HX/40HX up to +150, others +495). One value for all GPUs, or one per GPU in `--devices` order: `--uv-max 225,495,150` |

The miner reconnects to the pool on its own. Every 10 s it logs the hashrate per GPU and in total,
plus the submitted, accepted and rejected shares.

**Tuning.** For best efficiency on a 3070, lock the core clock (for example
`nvidia-smi -lgc 1560,1560`) and lower the memory clock (`nvidia-smi -lmc 810,810`): Pearl does not need a high memory
clock. At 1560 MHz a 3070 draws ~107 W. With the optional `--auto-uv` (new in 1.4.0) the miner also finds the lowest safe core voltage
of each GPU, checking every step bit-for-bit on the GPU and on the CPU, and restores your settings on exit
(roughly 5-10 W less per card at the same hashrate). Mixed rig: `--auto-uv` is enough, the limits are chosen per card
automatically (since 1.6.2); for manual control `--uv-max 225,495,150`. The work size is picked per GPU automatically, so
every card of a mixed rig runs at its own best rate (since 1.4.7).

**CPU.** The miner needs well under 10% of one CPU core per GPU, so multi-GPU rigs with older CPUs run at full speed.

**NOCK merge mining (LuckyPool).** Use `prl1...+YOUR_NOCK_ADDRESS` as the wallet and LuckyPool pays NOCK for the same work,
with no hashrate loss (Linux, Windows `.bat` and HiveOS alike; on other pools the miner drops the `+NOCK` part by itself).
**NACRE (NACR) merge mining (HimPool, AlphaPool):** `prl1...+nacr1p...` as the wallet (MANUAL.md). The step-by-step guide,
in English and Russian, with overclock settings is in **[HIVEOS.md](HIVEOS.md)**.

## NOID (Parano1d)

Own NOID kernels (closed source, `libnoidgpu.so` / `noidgpu.dll`; NVIDIA sm_80+, **driver 580+**), pool protocol
`parano1d-stratum-v1` over TLS, every candidate re-checked on the CPU with the official `noid_poseidon2b` crate, 1% dev fee.

```
./freeforgem --algo noid --pool stratum+ssl://eu2.innovlab.cc:19601 --wallet o1YOUR_ADDRESS --worker rig1 --lock-cclock 1300 --auto-uv
```

| RTX 3070, mem 810, PL 135 W | MH/s | W | MH/W |
|---|---|---|---|
| `--noid-mode eff` (default) @1400 MHz, +220 | 34.70 | 73.2 | **0.474** |
| `--noid-mode speed` @1560 MHz, +205 | 43.65 | 105.3 | 0.415 |
| `--noid-mode speed` @1650 MHz, +220 | **46.16** | 121.1 | 0.381 |

| 8x RTX 3070 rig, mem 810, `--auto-uv` (rig totals): mode / core lock | MH/s | W | MH/W |
|---|---|---|---|
| eff 1200 | 236.3 | 498 | 0.474 |
| eff 1300 ¹ | **257.0** | 523 | **0.491** |
| eff 1400 | 277.6 | 606 | 0.458 |
| eff 1500 | 295.3 | 682 | 0.433 |
| speed 1500 ¹ | **336.0** | 710 | **0.473** |
| speed 1560 | 349.1 | 756 | 0.462 |
| speed 1650 | 369.5 | 881 | 0.419 |
| speed 1750 | 393.0 | 1014 | 0.388 |

¹ 1.7.2 final (27.10.2026); the other rows: eff 1.7.2-beta2 (26.10), speed 1.7.2-beta4 (27.10), same rig and settings. The
1.7.2 tuning reaches +150..+240 at eff 1300 (+120..+150 before).

Recommended on RTX 30: **eff 1300** for the best MH/W (0.491), **speed 1500** (`--noid-mode speed --lock-cclock 1500`) for more
hashrate: +31 % MH/s for +36 % power (0.473 MH/W).

`--auto-uv` tunes a NOID-specific core offset per card and mode with bit-exact checks; without it a
positive (Pearl) offset is set to 0 for NOID. Windows: `start-noid.bat`. Details: [MANUAL.md](MANUAL.md), section 15.

## Common Foundry (CMFD)

Since 1.6.0 the same binary also mines **Common Foundry (CMFD, ForgeMatrix V4)**:
give it a CMFD pool URL and your CMFD address (`cmfd1...` as the wallet 1.0.16+ shows it, or the older 64-hex form), everything else works as for Pearl
(one process for all GPUs, `--devices`, `--lock-cclock/--lock-mclock/--moffset`, `--minutes`, the stats line, HiveOS, Windows):

```
./freeforgem --pool 'cmfd+tls://IP:PORT?pin=64HEX' --wallet cmfd1YOUR_ADDRESS --worker rig1
./freeforgem --algo cmfd --self-test
```

The CMFD work is done by the **FreeForgeMiner** engine (MIT, <https://github.com/pepsykolya/freeforgeminer>), shipped
in the `cmfd/` folder (release 1.2.5; the Linux binaries are built from the same source on Ubuntu 20.04, so CMFD runs on every
HiveOS): same consensus code, same speed (RTX 3070 ~20 FW/s, RTX 4070 Ti ~35 FW/s;
AMD RX 6800 XT ~10 FW/s on Windows). The 6.4 GB
model is downloaded once from the official Common Foundry servers and then only verified (`--cmfd-model DIR`; on HiveOS it is
shared with FreeForgeMiner in `/hive/miners/custom/cmfd-model`). Needs about 7 GB of free GPU memory and an RTX 20xx or newer.
`--cmfd-mode eco` = RTX 30 about 13% fewer watts for 2% less hashrate. A Pearl undervolt (positive core offset) makes CMFD
compute wrong results, so in CMFD mode the miner sets it to 0 for the run and puts it back on exit (needs root/Administrator;
`--uv-offset N` forces a value at your own risk). `--auto-uv` (NVIDIA) tunes a separate CMFD offset, every step checked
bit-for-bit on the real model (MANUAL.md, section 13).
**Use a separate OC profile for CMFD without the memory lock:** the Pearl OC (memory locked at 810 MHz) cuts CMFD about 6×
(RTX 3070 on HiveOS: 3.7 FW/s with *Mem lock* 810, 21.8 FW/s at ~237 W with the memory free); the miner warns about a low
memory clock in the log but never changes your OC.
Pools: **Aria** (default) `cmfd+tls://159.69.194.46:29445?pin=9dfb51083f287726117f689f87bc7a878792efcca58ca8b6f6e7c05ac5d152e9`,
**Midas Mining** `cmfd+tls://24.107.152.151:29745?pin=2ee4e6a028b07b300753dff3c260b79763c5e748d4e78b86cf3855e97eb2e80e`;
more in [MANUAL.md](MANUAL.md), section 13.
HiveOS: the same flight sheet with *Hash algorithm* `forgematrix_v4`, wallet template `%WAL%.%WORKER_NAME%` and the
`cmfd+tls://` pool URL; Windows: `start-cmfd.bat`. Details: [MANUAL.md](MANUAL.md) section 13 and `cmfd/README.md` in the package.

**CMFD dev fee: 1%**, inside the engine: 36 s per hour per GPU (random slot) on your pool, worker `ffm-fee`,
log lines `DEV FEE | ...`. The Pearl fee below is not run in CMFD mode.

## Proxima (PROX)

Since 1.7.8 the same binary mines **Proxima (PROX)** (BLAKE2b-256). Own CUDA kernel (closed source, `libproxgpu.so` / `proxgpu.dll`; NVIDIA sm_61+), every candidate hashed again on the CPU,
1% dev fee. Pools: proximamine (`stratum+ssl://pool.proximamine.com:3334`, password = your account secret), InnovLab
(`innov+tls://eu3.innovlab.cc:19701`, own protocol, password `x`), or solo against your own node (`--solo`, `--token`).

```
./freeforgem --algo prox --pool stratum+ssl://pool.proximamine.com:3334 --wallet ADDRESS --worker rig1 --password SECRET --auto-uv --lock-cclock 1500
```

The hashrate follows the core clock and PROX does not need the video memory (by default the miner locks the memory at a low clock
that still allows the full core range). So the **core lock sets the speed and the offset (undervolt) sets the watts**;
`--auto-uv` finds the offset of every card with bit-exact checks (a PROX-specific tuner, opt-in as for Pearl). Measured with a fixed
lock and offset, 0 wrong hashes at every point:

| GPU | `--lock-cclock` | offset | GH/s | W | GH/W |
|---|---|---|---|---|---|
| RTX 3070 | 1500 | +120 | 2.66 | 87 | 0.031 |
| RTX 3070 Laptop | 1500 | +270 | 2.23 | 53 | 0.042 |
| RTX 3080 Ti | 1500 | +210 | 4.40 | 147 | 0.030 |
| RTX 4070 Ti | 2450 | +270 | 5.47 | 142 | 0.0385 |

HiveOS: flight sheet with coin `PROX`, Hash algorithm left at `----`, Extra config `--auto-uv --lock-cclock 1500` (HIVEOS.md; screenshots
in MANUAL.md section 21). If the HiveOS OC (`nvidia-oc`) resets the core / memory lock or the offset, the miner sets them again (1.7.8).
Windows: `start-prox.bat`. Details: [MANUAL.md](MANUAL.md), section 21.

## NEB (Neural Engine Byte)

Since 1.8.0 the same binary mines **NEB** (nebhash v2) with the NEB module (the FFGN miner moved into FreeForGem; an FFGN command
line works unchanged with `--algo neb`). Engine nebgpu 0.8.1 (`libnebgpu.so` / `nebgpu.dll`, closed source) and the main-network
dataset `neb-datasets.bin` are in the packages; every nonce is checked again with the NEB C reference before it is sent. Unit MA/s
(10^6 attempts per second), efficiency MA/W.

* **Pool:** `stratum+tcp://neb.suprnova.cc:7804` (backup `:7805`), wallet `neb1...`. **Solo:** `--node 127.0.0.1:3441` (your NEB
  node's work service); **several rigs on one node:** `--gate` beside the node, the rigs use `--pool neb://GATE_IP:3442 --password TOKEN`.
* **Memory:** NEB is memory bound and runs at the full memory clock (a low lock of the HiveOS OC profile, e.g. Pearl's 810 MHz, is
  lifted while NEB mines and put back on exit). `--table-mclock 810` locks it low only where NEB does not need the fast memory (RTX 3070:
  -10 W, +8-10 % MA/W); `--tbig auto` (default) uses more GPU memory where it pays back.
* **One engine process per GPU:** a CUDA fault on one card restarts only that card, in a safe state (section 29 of the manual).
* **Optional power limit:** `--power-limit N` / `--pl N` (off by default; the miner's way is the core lock plus the offset).
* auto-uv / auto-lock as for the other coins; saved NEB profiles of FFGN are taken over on the first start.
* A 4-minute test of a 1.8.0 build (RTX 3070, Suprnova, lock 1680 MHz, +210, `--table-mclock 810`): about 165 MA/s at 118 W
  (**1.40 MA/W**); the rate changes with each block's model.

HiveOS: Hash algorithm `nebhash`, Pool URL `stratum+tcp://neb.suprnova.cc:7804`, Pass `x`. Windows: `start-neb.bat`. Options:
`freeforgem --algo neb --help`. Details: [MANUAL.md](MANUAL.md), section 24.

## Ting / Daler (DLR)

Since 1.8.0 the Ting miner FFGT (0.2.2) is a coin of FreeForGem: the same kernels and results, NVIDIA GTX 10 and newer (CUDA)
and AMD (OpenCL). Wallet = your Ting account (64 hex digits). Pool `crypto-connection.net:4073` (password `x`), your ffgt-gate
(`ting://GATE_IP:PORT`, the rig's password) or solo on a node of the rig (`--node 127.0.0.1:18440 --node-dir DIR`). Grouped search (default) gives about +40 % solo and +25 % on the pool on RTX 30
(`--no-group` turns it off); the memory is locked at its lowest clock by default (Ting does not need it); one engine process per GPU. RTX 3070: `--lock-cclock 1500` for
the best GH/W, `1800` for more GH/s. HiveOS: Hash algorithm left at `----`, coin `DLR`. Windows: `start-ting.bat`. Options:
`freeforgem --algo ting --help`. Details: [MANUAL.md](MANUAL.md), section 26.

## Different coins on different cards (`--split`)

```
./freeforgem --split neb:0,1 pearl:rest \
    --pool neb=stratum+tcp://neb.suprnova.cc:7804 --wallet neb=neb1YOUR_ADDRESS \
    --pool pearl=prl-eu.kryptex.network:7048 --wallet pearl=prl1YOUR_ADDRESS \
    --worker rig1 --auto-uv --lock-cclock neb=1800 --lock-cclock pearl=1410
```

* Groups `COIN:GPUS` (GPU numbers as in nvidia-smi, ranges `2-5`, one group may take `rest`); `;`, `,` and spaces are equal
  separators, no quotes needed.
* A value tagged with a coin (`--pool neb=URL`, `--wallet pearl=ADDR`, `--lock-cclock pearl=1410`) goes to that group only and
  **replaces the untagged value** of the option there; untagged options go to every group. A rig-wide per-GPU list
  (`--lock-cclock 1800,1800,1410,1410`) is sliced to each group's cards. `--split-args neb=--auto-uv` gives one group a flag
  (on HiveOS one word per `--split-args`, because HiveOS splits Extra config at spaces).
* One miner process per group, each with its own pool session, dev fee, clocks, memory policy, tuners and restarts: a group that
  ends restarts on its own while the others keep mining. Log lines carry the group in light blue: `[neb] [  60s] GPU0 ...`.
* **HiveOS:** the flight sheet's pool and wallet stay with the main coin, Extra config adds the other coin, e.g.
  `--split neb:0,1 pearl:2,3 --pool pearl=prl-eu.kryptex.network:7048 --wallet pearl=prl1YOUR_ADDRESS --lock-cclock pearl=1410`.
  The dashboard shows the algo and total of the group with the most cards, every card's own rate, the shares of all groups and
  every group's efficiency in the version column (`1.8.0 split: neb 1.40 MA/W + pearl 0.61 TH/W`).

Details: [MANUAL.md](MANUAL.md), section 27; step by step for HiveOS: [HIVEOS.md](HIVEOS.md).

## Developer fee

1% of the mining time: one 60-second slot in every 100 minutes, at a random moment of the cycle,
on **the same pool you configured**, under a separate login (worker `ffgp-devfee`; on LuckyPool it also merge-mines NOCK for the developer). The log shows
`devfee: next slot in N min`, `devfee: mining 60s for FreeForGem dev (1%)` and
`devfee: back to user`. The first slot comes at a random moment within the first 100 minutes. If the fee login
can't be served, the slot is skipped; your mining is never blocked.

Every coin pays 1 %, always on a connection of its own: Pearl and NOID a second login on your pool (above), PROX a second login on your pool too (proximamine only as the fallback),
CMFD inside the engine (36 s per hour per GPU), NEB on your pool list (solo / through a gate: the public NEB pools), Ting on
crypto-connection.net (NEB and Ting: worker `ffg`, 60 s in every 100 minutes). A slot whose fee connection was not ready is tried
again up to 3 times. With `--split` every group pays 1 % of its own cards' time.

The fee is a condition of the license: removing, reducing or redirecting it is not permitted
(see [LICENSE](LICENSE), clause 3).

## License

FreeForGem is distributed **in binary form only** (see [Releases](https://github.com/pepsykolya/FreeForGem/releases)) under the
**FreeForGem Binary License 1.0**: free to use and to redistribute the release packages unmodified, **as long as the developer
fee stays intact**. See [LICENSE](LICENSE). Versions up to and including 1.8.0 stay under the license they were published
with. Third-party components are listed in [NOTICE](NOTICE).
