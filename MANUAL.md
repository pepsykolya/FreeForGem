<!-- FreeForGemPear — see LICENSE -->
# FreeForGemPear — Manual

* [English](#english)
* [Русский](#русский)
* [中文](#中文)

---

## English

### 1. What it is

FreeForGemPear is a GPU miner for **Pearl (PRL)** (pearlhash proof-of-useful-work) on NVIDIA GPUs, with a **1% developer fee**.
It has its own CUDA kernel for Ampere: the A′ matrix is generated on-chip, the int8 tensor-core GEMM runs in a persistent
kernel, and the jackpot fold and the BLAKE3 hash are fused into it. The main loop is hand-tuned at SASS level.
The host builds and checks every share proof with the official `zk-pow` verifier before it is submitted.

| GPU series | Status |
|---|---|
| RTX 30xx (Ampere, sm_86): 3060 / 3060 Ti / 3070 / 3070 Ti / 3080 / 3090 | **main target, tuned: ~70 TH/s on an RTX 3070 at ~107 W** |
| RTX 40xx (Ada, sm_89) | runs the sm_86 code, untuned |
| RTX 50xx (Blackwell, sm_120) | supported since 1.5.0 with a compiled kernel (untuned); needs driver 570+ |
| Older GPUs, A100 (sm_80), AMD | not supported |

### 2. Requirements

* NVIDIA GPU of the RTX 30 series (RTX 40 runs untuned, RTX 50 since 1.5.0) and NVIDIA driver **525 or newer** (RTX 50xx: **570+**).
* HiveOS, Linux x86_64 (glibc 2.31+, Ubuntu 20.04+), or Windows 10/11 x64. No CUDA toolkit, no Python, no WSL.
* A PRL wallet address (`prl1...`) and an internet connection to a Pearl stratum pool.
* Video memory: the kernel works from the L2 cache and needs little VRAM.

### 3. HiveOS setup (step by step)

FreeForGemPear runs on HiveOS as a **Custom miner**: no SSH and no manual installation.

**Installation URL (1.4.9):**

```
https://github.com/pepsykolya/FreeForGemPear/releases/download/v1.5.1/freeforgempear-1.5.1.tar.gz
```

1. **Wallet.** In *Wallets*, add your PRL wallet: coin `PRL`, your `prl1...` address.
2. **New flight sheet.**
   * **Coin:** `PRL`.
   * **Wallet:** the wallet from step 1.
   * **Pool:** *Configure in miner*.
   * **Miner:** *Custom*, then press *Setup Miner Config*.
3. **Miner config:**

   | Field | Value |
   |---|---|
   | Miner name | `freeforgempear` (filled in automatically after you paste the URL) |
   | Installation URL | the link above |
   | Hash algorithm | `pearlhash` (or leave `----`) |
   | Wallet and worker template | `%WAL%.%WORKER_NAME%` |
   | Pool URL | `pearl-eu2.luckypool.io:3360`, or Kryptex `prl-eu.kryptex.network:7048` (any pool from section 6) |
   | Pass | leave empty |
   | Extra config arguments | optional, for example `--devices 0,1` |

4. **Apply.** *Apply changes*, *Create flight sheet*, then apply it to the rig.

The miner log is at `/var/log/miner/freeforgempear/freeforgempear.log` and in the HiveOS miner console.
The dashboard shows the hashrate per GPU and in total, temperatures, fans, and accepted and rejected shares.
To update, change the Installation URL to the new version. The same guide with more details: [HIVEOS.md](HIVEOS.md).

### 4. Linux without HiveOS

```
tar xzf freeforgempear-1.5.1-linux-x86_64.tar.gz
cd freeforgempear-1.5.1
./freeforgempear --self-test        # optional: checks the GPU kernel against the CPU reference
./freeforgempear --pool pearl-eu2.luckypool.io:3360 --wallet YOUR_PRL_ADDRESS --worker rig1     # LuckyPool
./freeforgempear --pool prl-eu.kryptex.network:7048 --wallet YOUR_PRL_ADDRESS --worker rig1       # Kryptex
```

| Option | Meaning |
|---|---|
| `--pool HOST:PORT` | stratum pool (default `pearl-eu2.luckypool.io:3360`) |
| `--wallet ADDRESS` | your PRL address (required) |
| `--worker NAME` | worker name (default `worker1`) |
| `--devices 0,1,...` | GPUs to use (default: all visible; honours `CUDA_VISIBLE_DEVICES`) |
| `--minutes N` | stop after N minutes (default: run until Ctrl-C / SIGTERM) |
| `--self-test` | verify the kernel and exit |

One process drives all GPUs and reconnects to the pool on its own. For a permanent setup run it under `systemd`, `screen` or `tmux`.

### 5. Windows 10/11

Native Windows build: no WSL, no CUDA installation, no Python. Only the NVIDIA driver is needed.

1. Download **`freeforgempear-1.5.1-windows-x64.zip`** from the release page:
   `https://github.com/pepsykolya/FreeForGemPear/releases/download/v1.5.1/freeforgempear-1.5.1-windows-x64.zip`
   and check its SHA-256 (section 9).
2. Unpack it to a folder **without spaces or non-English letters**, e.g. `C:\FreeForGemPear`.
3. Pick the start file for your pool — every one is ready to run, only the wallet has to be changed:

   | File | Pool |
   |---|---|
   | `start.bat`, `start-luckypool-eu.bat` | LuckyPool Europe `pearl-eu2.luckypool.io:3360` |
   | `start-luckypool-ru.bat` / `-us.bat` / `-asia.bat` | LuckyPool Russia / US East / Singapore |
   | `start-kryptex-eu.bat` | Kryptex Europe `prl-eu.kryptex.network:7048` |
   | `start-kryptex-ru.bat` / `-us.bat` / `-asia.bat` | Kryptex Russia / North America / Singapore |

   **Important:** in every file `WALLET` is preset to the **developer's example address**. Replace it with yours, or you mine for
   the developer (the miner warns about this at start). Right-click the file → *Edit* and set:

| Setting | Value |
|---|---|
| `WALLET` | your PRL address (`prl1...`) |
| `POOL` | `HOST:PORT`, default `pearl-eu2.luckypool.io:3360` (section 6) |
| `WORKER` | rig name, default = computer name |
| `DEVICES` | empty = all NVIDIA GPUs, or a list like `0,1` (indexes from `nvidia-smi`) |

4. Double-click the chosen **`.bat`** file. One miner process drives all GPUs; its output is shown in the window and saved to
   `logs\miner-<date>.log` (the last 10 logs are kept). If the miner exits or crashes, it is restarted after 15 s.
   `Ctrl+C` stops it.

* **Autostart:** put a shortcut to `start.bat` into `shell:startup` (Win+R, `shell:startup`), or create a Task Scheduler
  task "At log on" that runs `start.bat`.
* **Overclocking:** MSI Afterburner (core lock in the curve editor, memory down) with the values from section 7, **or**
  the miner's built-in clock control (next bullet). Laptops: plug in the charger and choose the maximum performance mode.
* **Built-in clock control (new in 1.4.9):** set `LOCK_CCLOCK`, `LOCK_MCLOCK` and/or `MOFFSET` in the `.bat` file (section 12),
  e.g. `set "LOCK_CCLOCK=1560"` and `set "LOCK_MCLOCK=810"`. The miner locks the clocks at start and restores them on exit.
* **Undervolt (optional):** remove `rem` before `set "AUTO_UV=1"` in `start.bat` and run it as Administrator; see section 12, *Windows: step by step*.
* **Run as Administrator for any GPU tuning.** `AUTO_UV`, `LOCK_CCLOCK`, `LOCK_MCLOCK` and `MOFFSET` change the GPU through the
  driver, which needs admin rights: right-click the `.bat` → **Run as administrator** (for autostart, tick *Run with highest
  privileges* in the Task Scheduler task). Without it the miner keeps running but mines at the current clocks and prints a
  warning. Plain mining (wallet / pool only) does **not** need Administrator.
* Defender / SmartScreen may flag a new unsigned `.exe`: *More info → Run anyway*, or add the folder to the exclusions.
* Without `start.bat`: `freeforgempear.exe --pool pearl-eu2.luckypool.io:3360 --wallet YOUR_PRL_ADDRESS --worker rig1`
* Build it yourself: `.\build-windows.ps1` (see the README).

**Advanced: your own `.bat` with `nvidia-smi` clock lock + power limit.** If you also want a **power limit** (the built-in
clock control does not set one), or prefer your own restart loop and log file, write a `.bat` like the one below and start it
**as Administrator** (right-click → *Run as administrator*; `nvidia-smi -lgc/-lmc/-pl` and `--auto-uv` all need admin):

```bat
@echo off
rem Lock clocks and power limit through the driver (Administrator; reset on reboot). See section 7 for values.
nvidia-smi -lgc 1665
nvidia-smi -lmc 810
nvidia-smi -pl 165
cd /d C:\Mining\freeforgempear-1.5.1-windows-x64
:loop
powershell -NoProfile -Command "& { .\freeforgempear.exe --pool pearl-eu2.luckypool.io:3360 --wallet YOUR_PRL_ADDRESS --worker rig1 --auto-uv --uv-min 135 --uv-max 360 | Tee-Object -Append -FilePath C:\Mining\ffgp.log }"
timeout /t 10
goto loop
```

Replace `YOUR_PRL_ADDRESS` with your `prl1...` address (for LuckyPool PRL+NOCK merge mining append `+YOUR_NOCK_ADDRESS`).
`-lgc` locks the core clock, `-lmc` the memory clock, `-pl` the power limit in watts; `timeout /t 10` + `goto loop` restart the
miner 10 s after any exit, and `Tee-Object` writes a log while still showing the output. To release the locks without a reboot:
`nvidia-smi -rgc` and `nvidia-smi -rmc`. (The shipped `start.bat` does the clock lock and restart loop for you via `LOCK_CCLOCK` /
`LOCK_MCLOCK` and `MOFFSET`; use a custom `.bat` like this only when you need `-pl` or your own logging.)
Since 1.5.1 the Windows archive ships this file ready to edit: **`start-nvidia-smi.bat`** (clock lock, power limit, undervolt
range, restart loop and `logsfgp.log` in the miner folder; the shipped values are for an RTX 3070, change them for your card).

### 6. Pools

Default: **LuckyPool**, `pearl-eu2.luckypool.io:3360` (plain TCP stratum). The hosts and ports below are taken from the pool's own
configuration (`https://pearl.luckypool.io/api/stats`, `config.stratum` and `config.ports`); pick the region closest to you:

| Region | Host |
|---|---|
| Europe | `pearl-eu2.luckypool.io` (default), `pearl-eu1.luckypool.io`, `pearl-pl.luckypool.io` (Poland), `pearl-tr.luckypool.io` (Turkey) |
| Russia | `pearl-ru.luckypool.io` |
| US | `pearl-us-east.luckypool.io`, `pearl-us-west.luckypool.io`, `pearl-us-central.luckypool.io`, `pearl-us-ord.luckypool.io` |
| South America | `pearl-br.luckypool.io` (Brazil) |
| Canada | `pearl-ca1.luckypool.io`, `pearl-ca2.luckypool.io` |
| Asia | `pearl-sg1.luckypool.io`, `pearl-sg2.luckypool.io` (Singapore), `pearl-id.luckypool.io` (Indonesia), `pearl-hk.luckypool.io` (Hong Kong), `pearl-in.luckypool.io` (India), `pearl-jp.luckypool.io` (Japan), `pearl-au.luckypool.io` (Australia) |

| Port | Share difficulty (vardiff) |
|---|---|
| `3360` | 2 000 000 (default) |
| `3361` | 4 000 000 |
| `3362` | 8 000 000 |

Example: `--pool pearl-pl.luckypool.io:3360`. Use your plain `prl1...` address as the wallet; the worker name is sent separately.
Any other Pearl stratum pool that speaks the same JSON-RPC stratum can be used; the dev fee (section 10) is then mined on that pool.

**NOCK merge mining (LuckyPool only).** LuckyPool pays Nockchain (NOCK) for the same Pearl work, with no
hashrate loss: write the wallet as `PRL_ADDRESS+NOCK_ADDRESS`, for example
`--wallet prl1...+YOUR_NOCK_ADDRESS`. NOCK stats are at `https://nock.luckypool.io` under your NOCK address. In the Windows
`.bat` files put the same `prl1...+NOCK...` into `WALLET`; on HiveOS put it into the PRL wallet address. Other pools
(Kryptex) do not support it: since 1.4.3 the miner drops the `+NOCK` part there by itself (log: `PRL+NOCK merge mining works on
LuckyPool only`) and mines PRL to the `prl1...` part.

**Kryptex** (checked in 1.4.1): `--pool prl-eu.kryptex.network:7048` (also `prl`, `prl-us`, `prl-br`, `prl-sg`, `prl-hk`, `prl-ru`,
`prl-ae` `.kryptex.network`; port 7048 is plain TCP). Wallet: your `prl1...` address. Kryptex reads the worker name from the login,
so since 1.4.3 the miner sends `prl1....WORKER` there (before 1.4.3 Kryptex showed every rig as `worker`).
Big rigs on Kryptex (more than ~0.5 PH/s per worker) can set a custom share difficulty since 1.4.5: `--password d=VALUE`
(HiveOS: Extra config arguments; Windows: `PASSWORD` in the `.bat`), `VALUE = hashrate (H/s) × seconds per share / 4294967296`;
the default is 2097152 (a 100 TH/s worker finds a share every ~90 s).

**Proof compression.** Proofs are sent gzip-compressed (about 5 KB instead of about 270 KB: much less traffic, faster submits,
fewer stale shares) when the pool agrees to it at login: since 1.4.4 the miner follows the Pearl stratum gzip protocol (sends
`"type":"v2"` in `mining.authorize`; Kryptex and LuckyPool answer `"type":"v2"`, log: `pool confirmed gzip proofs`). A pool that does
not confirm it gets uncompressed proofs. Since 1.4.5 the proof is compressed at the standard gzip level (~3 KB) and jobs that
arrive before the login reply wait for it, so every proof follows the agreed format. `FFGP_PROOF_GZIP=on` / `off` forces the choice.
**Kryptex users of 1.4.1–1.4.3:** those versions sent compressed proofs without this handshake; Kryptex answered "accepted" but did
not credit the shares. Update to 1.4.4 or newer.

### 7. Overclocking (core lock, memory, V/F offset)

The kernel works from the L2 cache and needs almost no VRAM bandwidth, so the memory clock can go down to the lowest value.
The three settings that matter: a **locked core clock**, a **low memory clock**, and a **V/F offset** (undervolt: the same
clock at a lower voltage; `--auto-uv` finds it for you, section 12).

**Measured with FreeForGemPear, RTX 3070:** core `1560` MHz locked, memory `810` MHz, power limit `120` W →
**~70 TH/s at ~105–107 W**; `--auto-uv` then finds +195..240 (≈ 86–107 W).

**Starting values per card.** *Reference* rows are published tuning results for that card; *estimate* rows are derived from
cards of the same generation and are not measured. Every card differs: let `--auto-uv` find your own offset.

| GPU | Core lock, MHz | Memory | V/F offset, MHz | Source |
|---|---|---|---|---|
| RTX 3060 Ti, RTX 3070, RTX 3070 Laptop, RTX 3080 Ti | 1560–1650 | lock `810` | +200..255 | reference (3070: measured) |
| RTX 3060, RTX 3070 Ti, RTX 3080, RTX 3090 | 1560–1650 | lock `810` | +200..255 | estimate |
| CMP 70HX, CMP 90HX (Ampere) | 1650 | offset `-2000` (`--moffset`) | +255..300 | reference, not checked with FreeForGemPear |
| RTX 4070 Ti, RTX 4090 | 2445 | lock `810` | +315..345 | reference |
| RTX 4060, 4060 Ti, 4070, 4070 Super, 4070 Ti Super, 4080, 4080 Super | 1900–2445 | lock `810` | +300..345 | estimate |
| RTX 5070 Ti | 2445 | lock `810` | +480 | reference |
| RTX 5070 | 2400 | lock `810` | checks pass up to +390, crash above | user report, FreeForGemPear 1.5.1, Windows, driver 617.14 |
| RTX 5060, 5060 Ti, 5080, 5090 | 1900–2500 | lock `810` | +390..480 | estimate |

RTX 40 and 50 run at a much higher locked core clock (**1900–2500 MHz**) and hold a higher offset than RTX 30; that is why
auto-uv starts their search at +200 (RTX 30: +120). Turing / CMP 30HX–50HX (sm_75) and CMP 170HX / A100 (sm_80) are not supported.

* HiveOS: Overclock tab → core clock (absolute), memory clock (absolute), power limit; the V/F offset via `--auto-uv`.
* Linux: `nvidia-smi -lgc 1560,1560`, `nvidia-smi -lmc 810,810`, `nvidia-smi -pl 120` (root), or `--lock-cclock` / `--lock-mclock`.
* Windows: `LOCK_CCLOCK` / `LOCK_MCLOCK` / `MOFFSET` in `start.bat` (as Administrator), `start-nvidia-smi.bat` for a power
  limit, or MSI Afterburner. Ready combinations per card: section 12, *Windows: ready combinations*.

If shares get rejected or the log shows `bad` hits, lower the core clock or the offset.

### 8. Reading the log

| Line | Meaning |
|---|---|
| `FreeForGemPear 1.4.9 \| pool ... \| worker ... \| N GPU(s)` | start banner |
| `[pearlgpu] search kernel: SASS-patched cubin (...)` | the tuned kernel is loaded. `embedded search kernel could not be loaded (reason)` = the miner stops with an error (driver older than 525 or an unsupported GPU) |
| `new job ... prepared in ...` | a new pool job is ready |
| `[ 120s] GPU0 ... \| total X TH/s \| hits ... \| user sub S acc A rej R \| fee sub ... acc ... rej ...` | every 10 s: hashrate per GPU and in total; shares submitted / accepted / rejected for you and for the dev fee; `[DEVFEE]` while a fee slot runs |
| `[DISCONNECTED]` | the pool connection is down; the miner reconnects by itself |
| `devfee: ...` | the dev fee slots (section 10) |
| `!! ...` | a warning or error (a GPU worker stopped, a hit was not submitted) |

### 9. Verifying the download (SHA-256)

Compare the result with the hash on the release page.

* Linux: `sha256sum freeforgempear-1.5.1-linux-x86_64.tar.gz freeforgempear-1.5.1.tar.gz`
* Windows: `certutil -hashfile freeforgempear-1.5.1-windows-x64.zip SHA256` or
  `Get-FileHash freeforgempear-1.5.1-windows-x64.zip -Algorithm SHA256`

### 10. Dev fee — 1%

One 60-second slot in every 100 minutes, at a random moment of the cycle, on **the same pool you configured**, under a
separate login (worker `ffgp-devfee`). The log shows `devfee: next slot in N min`,
`devfee: mining 60s for FreeForGemPear dev (1%)` and `devfee: back to user`. The first slot comes at a random moment within
the first 100 minutes. If the fee login cannot be served, the slot is skipped; your mining is never blocked.
On LuckyPool the fee login also merge-mines NOCK for the developer (`prl1...+NOCK`, since 1.4.2); if the pool refuses that login,
the miner falls back to the plain PRL fee wallet. The fee time stays 1%.
The fee is a condition of the license: removing, reducing or redirecting it is not permitted (see [LICENSE](LICENSE), clause 3).

### 11. Troubleshooting

| Symptom | Fix |
|---|---|
| Check that everything works | `freeforgempear --self-test` (Windows: `freeforgempear.exe --self-test`) must end with `== self-test PASSED ==` |
| `embedded search kernel could not be loaded` at start | update the NVIDIA driver (525+; **RTX 50xx needs 570+**) and check that the GPU is Ampere (RTX 30xx) or newer |
| High CPU load | by default GPU waits sleep (well under 10% of a core per GPU). `PEARL_SPIN=1` makes them spin (a full core per GPU): make sure it is not set |
| `pg_init(...)` or out-of-memory errors | another program holds the GPU (games, local AI models). Close it or choose GPUs with `--devices` |
| `[DISCONNECTED]` for a long time | check the pool host/port and the firewall (outgoing TCP); try another region from section 6 |
| Many rejected shares | lower the core clock (section 7), check cooling |
| HiveOS says `Already installed` and nothing changes | the Installation URL must point to a new version |
| Windows: the window closes at once | run `start.bat` from `cmd` to see the message; the path must not contain spaces or non-English letters |
| Windows: `.exe` blocked by Defender / SmartScreen | *More info → Run anyway*, or add the folder to the exclusions; check the SHA-256 |

Environment variables (set before starting the miner): `PEARL_SPIN=1` (spin-wait instead of sleeping).
`FFGP_M=N` sets the rows of A per attempt (a multiple of 128). By default `m` is 47104 and the miner picks `m` for each GPU from its SM count (`m = 128 * SM * r`), so a mixed-SM rig balances every card — one job per SM group, each card mines its own (new in 1.4.7). Set `FFGP_M` only to experiment: it forces one `m` on every device, and a larger `m` gives slightly higher throughput at the cost of more host RAM.
On Windows: `set PEARL_SPIN=1` in a `cmd` window, then run `start.bat` from that window.

### 12. Undervolt / `--auto-uv` (optional, new in 1.4.0)

**Off by default.** With `--auto-uv` the miner finds, once per GPU, a lower core voltage (the GPU's *V/F offset*) at which the
card still computes everything correctly, uses it while mining, and puts your own settings back when it exits. Lower voltage
means fewer watts at the same hashrate: the clock stays where you locked it (section 7).

**What it does**
1. Reads the V/F offset the card has now (HiveOS OC, your own script, ...) and writes it to a restore file *before* changing anything.
2. Warms the GPU up for 3 minutes, then raises the offset in steps of **15 MHz**. Every step is checked for about 30 s in a
   separate process: the same job is computed thousands of times and every tile of every result is compared bit for bit,
   the hit counts of normal mining attempts are compared, and found results are verified on the CPU with the official verifier.
   Wrong results without any driver error do happen near the limit, which is why the checks are done on the GPU *and* the CPU.
3. The first mismatch, CUDA error, Xid error or hang ends the climb. The result is the **last good step minus one step (15 MHz
   margin)**, confirmed by a longer check (about 2 minutes); if the confirmation fails it goes one step lower. The highest
   value tried is +495 (`--uv-max`, at most +600; before 1.5.1 it was +300 with a hard cap of +400). RTX 40/50 cards often
   hold +450..480.
4. The result is saved in `uv.json`, keyed by GPU UUID, driver version and kernel build. Later starts apply it after a
   quick check (about 30 s). If the driver, the kernel build or your starting offset changed, the GPU is tuned again.
5. While mining: every 6 hours (`--uv-reval-hours`) the setting is re-checked (mining on that GPU pauses for about 30 s).
   If a found result fails the CPU verification, or a burst of rejected shares appears, the offset goes one step down (never
   below your own value) and is saved. If somebody else changes the offset (for example HiveOS re-applies its OC), auto-uv
   stands down on that GPU and does not fight.
6. On exit (Ctrl+C, SIGTERM, normal end) your offset is restored. If the miner was killed (`kill -9`, crash), the next start
   restores your offset first, from the restore file `uv-restore.json`. A reboot resets the offset in the driver anyway.

Only the V/F offset is changed: locked clocks, power limit and memory are not touched. Only the GPUs the miner mines on are
touched. NVML device numbers differ from CUDA numbers, so every GPU is matched by its PCI bus id (and UUID); the log shows the
bus id of each change.

**Requirements.** NVIDIA driver (NVML is loaded at run time, nothing to install) and **root** on Linux / HiveOS (the miner
runs as root there) or **Administrator** on Windows. Without NVML or permission the miner logs the reason and mines normally.
**Time:** the first tuning takes about **15 minutes per GPU** (10-25 depending on how many steps it takes; the GPUs of a rig are tuned in parallel and mining starts when
it is done). Later starts add about 30 s.

**Windows: step by step.** Checked on Windows 11 with a GeForce RTX 3070 and driver 581.80: the driver lets the miner read
and change the offset (the log shows `allowed -1000..1000`), the tuner found +240 instead of +225 (98.4 W → 86.5 W at the
same clock), and the safety net worked too (a found result failed the CPU check → back to +225, auto-uv paused for 24 h).
1. **Administrator.** Right-click the `.bat` → *Run as administrator*. For autostart create a Task Scheduler task with
   *Run with highest privileges*. Without it the log says that the offset cannot be changed, and the miner just mines.
2. **Lock the core clock** (section 7): `set "LOCK_CCLOCK=1560"` in `start.bat` (one value, or a comma list per GPU), or
   `nvidia-smi -lgc` in `start-nvidia-smi.bat`. With a locked clock a higher offset means a lower voltage at the same clock;
   without the lock the card may boost higher instead and the saving is smaller.
3. **Turn it on**, one of:
   * `start.bat` (or any `start-luckypool-*` / `start-kryptex-*`): remove `rem` before `set "AUTO_UV=1"`. The upper limit is
     `set "UV_MAX="` (empty = +495; RTX 40/50 often hold +450..480, RTX 30 usually +200..360).
   * `start-nvidia-smi.bat`: `AUTO_UV=1`, the range in `UV_MIN` / `UV_MAX`, plus power limit and clock lock through `nvidia-smi`.
   * your own command line: `freeforgempear.exe --pool ... --wallet ... --auto-uv --uv-max 480`.
4. **Starting point.** The tuner starts from the offset the card has now and never goes below it. The *Core Clock* offset of
   MSI Afterburner (or a similar tool) is this same offset, so a value you set there is your starting point and is put back
   on exit. Do not let Afterburner re-apply a profile while mining: auto-uv sees that somebody else changed the offset and
   stands down on that GPU. With no offset set the tuner starts at +120, on RTX 40/50 at +200 (or `--uv-min` / `UV_MIN`).
5. **Wait.** The first start tunes for about 15 minutes per GPU (all GPUs in parallel) and only then starts mining. Later
   starts apply the saved value after a ~30 s check.
6. **Files** are in the miner folder: `uv.json` (results) and `uv-restore.json` (your original offset). The folder must be
   writable, so do not put the miner into `C:\Program Files`. To tune again: close the miner, delete `uv.json` (or run
   `freeforgempear.exe --uv-reset` in that folder) and start again. A new driver or a new miner kernel triggers a new tuning by itself.
7. **Read the result:** `findstr auto-uv logs\*.log` (`start-nvidia-smi.bat`: `logs\ffgp.log`). Example from that RTX 3070:
```
auto-uv: GPU0 (bus 04, RTX 3070): CUDA device 0 -> NVML index 0, PCI 0000:04:00.0, ..., current offset +225 (allowed -1000..1000)
auto-uv: GPU0 (bus 04, RTX 3070) step +255: ok (6560 reruns, cpu ok 6), 94.0 W, 1560 MHz, 58 C
auto-uv: GPU0 (bus 04, RTX 3070) step +270: FAILED (rerun_diff: 256 transcript word(s) differ between reruns of seed 3 ...)
auto-uv: GPU0 (bus 04, RTX 3070) offset +225 -> +240, 98.4 W -> 86.5 W, saved
```
   * `auto-uv paused for this GPU for 24 h`: the card had an error at the found offset; it mines at your own offset and
     the tuner tries again after 24 h (or after `--uv-reset`).
   * `start offset ... leaves no room below the cap`: your offset is already at `UV_MAX` or higher; raise `UV_MAX`.
   * `no safe offset above ... was found`: the card does not take more than your own value; nothing is changed.
8. **If the PC hangs or the driver resets** (black screen, "display driver stopped responding"): reboot and start the miner
   again; it first restores your offset from `uv-restore.json`. If the miner itself crashed during a tuning step (window shows
   `Miner exited (code -1073741819)`), the next start counts that step as failed and tunes only below it (since 1.5.2; before,
   it started the whole search again and crashed at the same step). If a card keeps failing, lower `UV_MAX` or leave `AUTO_UV` off.

**Windows: ready combinations.** Right-click `start.bat` → *Edit*, remove `rem` before `set "AUTO_UV=1"`, fill in the
fields below, save, and start it **as Administrator**. Empty fields keep their defaults. Values are starting points from
the table in section 7.
```bat
rem --- RTX 30 (3060 Ti / 3070 / 3080 ...) ---
set "AUTO_UV=1"
set "LOCK_CCLOCK=1560"
set "LOCK_MCLOCK=810"
set "UV_MIN=150"
set "UV_MAX=360"

rem --- RTX 40 (4060 ... 4090) ---
set "AUTO_UV=1"
set "LOCK_CCLOCK=2445"
set "LOCK_MCLOCK=810"
set "UV_MIN=200"
set "UV_MAX=450"

rem --- RTX 50 (5060 ... 5090): start higher, the search is shorter ---
set "AUTO_UV=1"
set "LOCK_CCLOCK=2445"
set "LOCK_MCLOCK=810"
set "UV_MIN=300"
set "UV_MAX=495"
```
* **Mixed rig:** `LOCK_CCLOCK` / `LOCK_MCLOCK` take one value per GPU in `DEVICES` order, e.g. `set "LOCK_CCLOCK=1560,2445"`.
  `UV_MIN` / `UV_MAX` are common to all GPUs: leave them empty (RTX 40/50 start at +200 by themselves) or use the lower card's values.
* **Faster tuning:** `set "UV_STEP=30"` halves the number of steps (coarser result). On RTX 50 a higher `UV_MIN` (about +300)
  saves the most time: the card holds it anyway, and the search starts there. Do not set it too high: if the card fails
  the check already at `UV_MIN`, it is not tuned and stays at your own offset (the log says so; lower `UV_MIN` then).
* **Already know your offset:** `set "UV_OFFSET=480"` (and `AUTO_UV` left off) applies it after one quick check, no search.
* **Power limit as well:** use `start-nvidia-smi.bat` (`CCLOCK` / `MCLOCK` / `PLIMIT`, `UV_MIN` / `UV_MAX` / `UV_STEP`).
* **Search again after changing the range:** add `set "EXTRA=--uv-reset"` for one start, then empty it again.

**Risk.** Undervolting beyond what a card can take makes a driver reset or a hang possible (the tuner only goes where it
has just checked, with a margin, but silicon, temperature and driver differ). After a hang, reboot: the next start restores your
offset first. Use it at your own risk, as with any overclocking.

**Enable**
* HiveOS: put `--auto-uv` into *Extra config arguments* of the flight sheet (details in HIVEOS.md).
* Linux: `sudo ./freeforgempear --pool ... --wallet ... --auto-uv`
* Windows: right-click `start.bat` → *Edit* and remove `rem` before `set "AUTO_UV=1"`; start `start.bat` **as Administrator**.

| Option | Meaning |
|---|---|
| `--auto-uv` | find, apply and supervise the undervolt (as described above) |
| `--uv-offset N` | apply the fixed offset N MHz after a quick check, no tuning (if the check fails, your offset stays) |
| `--uv-reset` | forget the saved results; with `--auto-uv` the GPUs are tuned again; without a wallet the miner just exits |
| `--uv-max N` | highest offset the tuner tries (default 495, at most 600; on Windows: `set "UV_MAX=480"` in `start.bat`) |
| `--uv-min N` | offset the tuner starts from and never goes below (default: the HiveOS/driver offset, else 120; RTX 40/50: at least 200; Windows: `UV_MIN`) |
| `--uv-step N` | MHz per tuning step (default 15; Windows: `UV_STEP`) |
| `--uv-keep-last` | after a failed step keep the last good step (default: the last good step minus one step) |
| `--uv-reval-hours H` | re-validation interval while mining (default 6, `0` = never) |
| `--uv-dir DIR` | where `uv.json` and `uv-restore.json` are kept (also `FFGP_STATE_DIR`) |
| `--auto-uv-dry-run` | only logs which GPUs would be changed (CUDA device → NVML device, bus id); changes nothing |

**GPU clock control (optional, NVML; needs root / Administrator, works on Windows — new in 1.4.9).** `--lock-cclock N` locks the core clock to N MHz (like `nvidia-smi -lgc N,N`), `--lock-mclock N` locks the memory clock to N MHz (like `-lmc`), and `--moffset N` sets a memory-clock offset in MHz, which may be negative (needs driver 555+). Each takes one value for all GPUs, or a comma-separated list in `--devices` order (one value per device). The settings are applied at start and undone on exit (Ctrl-C / SIGTERM / crash). On Windows set `LOCK_CCLOCK`, `LOCK_MCLOCK`, `MOFFSET` in the `.bat` file. Example: `--lock-cclock 1500 --lock-mclock 810 --moffset -200`.

Default location of the files: next to the binary (Windows: the miner folder), on HiveOS `/hive-config/freeforgempear`
(survives miner updates and reboots). **Reset:** `freeforgempear --uv-reset` (or delete `uv.json`).

**Log lines** (all start with `auto-uv:`, so `grep auto-uv` shows the story):
```
auto-uv: GPU0 (bus 0d, RTX 3070): CUDA device 0 -> NVML index 1, PCI 0000:0D:00.0, GPU-0d06..., current offset +150
auto-uv: GPU0 (bus 0d, RTX 3070) step +195: ok (1952 reruns, cpu ok 6), 110.4 W, 1560 MHz, 53 C
auto-uv: GPU0 (bus 0d, RTX 3070) step +225: FAILED (rerun_diff: ...)
auto-uv: GPU0 (bus 0d, RTX 3070) offset +150 -> +195, 116.6 W -> 106.6 W, saved
auto-uv: GPU0 (bus 0d, RTX 3070) offset +195 -> +150 restored
```

**Reference values, RTX 3070 locked at 1560 MHz / 810 MHz memory** (~70 TH/s at every offset, the hashrate does not change):
about 116 W at +150, 113 W at +180, 110 W at +195 and 107–108 W at +210. On our test card the built-in check found silent
calculation errors from +225 upward, although a shorter manual test had passed even +255, so the tuner settled at +195...+210.
Silent errors show no Xid and no rejected shares, but they lose valid shares, so a higher offset that "looks stable" can earn less.
The edge differs from chip to chip and with temperature: expect roughly 5–10 W per card compared with a stock offset, verify your
value with `--uv-offset N` and do not copy another card's number.

**Manual tuning (without `--auto-uv`)**, for those who prefer the HiveOS OC tab or Afterburner:
1. Lock the core clock and lower the memory clock as in section 7, and let the card warm up for a few minutes.
2. Raise the core clock **offset** (the V/F offset: HiveOS Overclock tab, Afterburner, or `--uv-offset N`) by 15 MHz.
3. Run at least 30 minutes and watch: `bad` must stay 0, `rej` must not grow, the log must not show `DEAD`, and `dmesg` / the
   HiveOS messages must show no `Xid` (13, 31, 43, 79 ...). `freeforgempear --uv-offset N` applies N, runs the same check as
   `--auto-uv` and mines with it, which is a quick way to test a candidate value.
4. Repeat from step 2 until the first problem, then go back **two steps (30 MHz)** from the offset where it appeared and keep that.
5. Test again after a driver update, a new cooler or hot weather.

---

## Русский

### 1. Что это

FreeForGemPear — GPU-майнер **Pearl (PRL)** (pearlhash, proof-of-useful-work) для видеокарт NVIDIA с **комиссией разработчика 1%**.
Собственное CUDA-ядро для Ampere: матрица A′ генерируется на кристалле, int8-GEMM на тензорных ядрах идёт в
персистентном ядре, а свёртка jackpot и хеш BLAKE3 слиты с ним. Главный цикл настроен вручную на уровне SASS.
Хост строит и проверяет доказательство каждой шары официальным верификатором `zk-pow` до отправки.

| Серия | Статус |
|---|---|
| RTX 30xx (Ampere, sm_86): 3060 / 3060 Ti / 3070 / 3070 Ti / 3080 / 3090 | **основная цель, оптимизировано: ~70 TH/s на RTX 3070 при ~107 Вт** |
| RTX 40xx (Ada, sm_89) | запускает код sm_86, без оптимизации |
| RTX 50xx (Blackwell, sm_120) | поддержка с 1.5.0, компилированное ядро (без оптимизации); нужен драйвер 570+ |
| Старые карты, A100 (sm_80), AMD | не поддерживаются |

### 2. Требования

* Видеокарта NVIDIA серии RTX 30 (RTX 40 — без оптимизации, RTX 50 — с 1.5.0), драйвер NVIDIA **525 или новее** (RTX 50xx: **570+**).
* HiveOS, Linux x86_64 (glibc 2.31+, Ubuntu 20.04+) или Windows 10/11 x64. CUDA toolkit, Python и WSL не нужны.
* Адрес кошелька PRL (`prl1...`) и интернет до stratum-пула Pearl.
* Видеопамять: ядро работает из L2-кэша и почти не использует VRAM.

### 3. Настройка в HiveOS (по шагам)

FreeForGemPear запускается в HiveOS как **Custom miner**: SSH и ручная установка не нужны.

**Ссылка для установки (1.4.9):**

```
https://github.com/pepsykolya/FreeForGemPear/releases/download/v1.5.1/freeforgempear-1.5.1.tar.gz
```

1. **Кошелёк.** В разделе *Wallets* добавьте кошелёк PRL: монета `PRL`, адрес `prl1...`.
2. **Новый полётный лист.**
   * **Coin:** `PRL`.
   * **Wallet:** кошелёк из шага 1.
   * **Pool:** *Configure in miner*.
   * **Miner:** *Custom*, затем *Setup Miner Config*.
3. **Настройки майнера:**

   | Поле | Значение |
   |---|---|
   | Miner name | `freeforgempear` (подставится сам после вставки ссылки) |
   | Installation URL | ссылка выше |
   | Hash algorithm | `pearlhash` (или оставить `----`) |
   | Wallet and worker template | `%WAL%.%WORKER_NAME%` |
   | Pool URL | `pearl-eu2.luckypool.io:3360` или Kryptex `prl-eu.kryptex.network:7048` (любой пул из раздела 6) |
   | Pass | пусто |
   | Extra config arguments | по желанию, например `--devices 0,1` |

4. **Применить.** *Apply changes*, *Create flight sheet*, затем применить лист к ригу.

Лог майнера: `/var/log/miner/freeforgempear/freeforgempear.log` и консоль майнера в HiveOS.
На панели видны хэшрейт по картам и общий, температура, вентиляторы, принятые и отклонённые шары.
Для обновления смените Installation URL на новую версию. То же подробнее: [HIVEOS.md](HIVEOS.md).

### 4. Linux без HiveOS

```
tar xzf freeforgempear-1.5.1-linux-x86_64.tar.gz
cd freeforgempear-1.5.1
./freeforgempear --self-test        # по желанию: сверка GPU-ядра с CPU-эталоном
./freeforgempear --pool pearl-eu2.luckypool.io:3360 --wallet ВАШ_АДРЕС_PRL --worker rig1     # LuckyPool
./freeforgempear --pool prl-eu.kryptex.network:7048 --wallet ВАШ_АДРЕС_PRL --worker rig1       # Kryptex
```

| Опция | Значение |
|---|---|
| `--pool HOST:PORT` | stratum-пул (по умолчанию `pearl-eu2.luckypool.io:3360`) |
| `--wallet ADDRESS` | ваш адрес PRL (обязательно) |
| `--worker NAME` | имя воркера (по умолчанию `worker1`) |
| `--devices 0,1,...` | какие GPU использовать (по умолчанию все видимые; учитывает `CUDA_VISIBLE_DEVICES`) |
| `--minutes N` | остановиться через N минут (по умолчанию работает до Ctrl-C / SIGTERM) |
| `--self-test` | проверить ядро и выйти |

Один процесс ведёт все GPU и сам переподключается к пулу. Для постоянной работы запускайте под `systemd`, `screen` или `tmux`.

### 5. Windows 10/11

Нативная сборка: WSL, установка CUDA и Python не нужны. Нужен только драйвер NVIDIA.

1. Скачайте **`freeforgempear-1.5.1-windows-x64.zip`** со страницы релиза:
   `https://github.com/pepsykolya/FreeForGemPear/releases/download/v1.5.1/freeforgempear-1.5.1-windows-x64.zip`
   и сверьте SHA-256 (раздел 9).
2. Распакуйте в папку **без пробелов и русских букв**, например `C:\FreeForGemPear`.
3. Выберите файл запуска под свой пул — все готовы к работе, поменять нужно только кошелёк:

   | Файл | Пул |
   |---|---|
   | `start.bat`, `start-luckypool-eu.bat` | LuckyPool Европа `pearl-eu2.luckypool.io:3360` |
   | `start-luckypool-ru.bat` / `-us.bat` / `-asia.bat` | LuckyPool Россия / США (восток) / Сингапур |
   | `start-kryptex-eu.bat` | Kryptex Европа `prl-eu.kryptex.network:7048` |
   | `start-kryptex-ru.bat` / `-us.bat` / `-asia.bat` | Kryptex Россия / Северная Америка / Сингапур |

   **Важно:** во всех файлах `WALLET` заранее заполнен **адресом разработчика из примера**. Замените его своим, иначе вы майните
   на разработчика (майнер предупредит об этом при старте). Правой кнопкой по файлу → *Изменить* и задайте:

| Параметр | Значение |
|---|---|
| `WALLET` | ваш адрес PRL (`prl1...`) |
| `POOL` | `HOST:PORT`, по умолчанию `pearl-eu2.luckypool.io:3360` (раздел 6) |
| `WORKER` | имя рига, по умолчанию имя компьютера |
| `DEVICES` | пусто = все GPU NVIDIA, или список вида `0,1` (индексы из `nvidia-smi`) |

4. Запустите выбранный **`.bat`**. Один процесс майнера ведёт все GPU; вывод идёт в окно и в файл
   `logs\miner-<дата>.log` (хранятся последние 10 логов). Если майнер завершился или упал, он перезапускается через 15 с.
   `Ctrl+C` — остановка.

* **Автозапуск:** положите ярлык `start.bat` в `shell:startup` (Win+R, `shell:startup`) или создайте задачу в Планировщике
  «При входе в систему», запускающую `start.bat`.
* **Разгон:** MSI Afterburner (фиксация ядра в редакторе кривой, память вниз) со значениями из раздела 7 **или** встроенное
  управление частотами (следующий пункт). Ноутбуки: подключите зарядку и выберите режим максимальной производительности.
* **Встроенное управление частотами (с 1.4.9):** задайте `LOCK_CCLOCK`, `LOCK_MCLOCK` и/или `MOFFSET` в `.bat`-файле (раздел 12),
  например `set "LOCK_CCLOCK=1560"` и `set "LOCK_MCLOCK=810"`. Майнер фиксирует частоты при старте и возвращает их при выходе.
* **Андервольт (необязательно):** уберите `rem` перед `set "AUTO_UV=1"` в `start.bat` и запускайте его от администратора; см. раздел 12, *Windows: по шагам*.
* **Запуск от администратора для любой настройки GPU.** `AUTO_UV`, `LOCK_CCLOCK`, `LOCK_MCLOCK` и `MOFFSET` меняют GPU через
  драйвер, а для этого нужны права администратора: правой кнопкой по `.bat` → **Запуск от имени администратора** (для
  автозапуска включите *Выполнять с наивысшими правами* в задаче Планировщика). Без этого майнер продолжит работать, но на
  текущих частотах и с предупреждением в логе. Для обычного майнинга (только кошелёк/пул) администратор **не** нужен.
* Defender / SmartScreen может ругаться на новый неподписанный `.exe`: *Подробнее → Выполнить в любом случае* или добавьте папку в исключения.
* Без `start.bat`: `freeforgempear.exe --pool pearl-eu2.luckypool.io:3360 --wallet ВАШ_АДРЕС_PRL --worker rig1`
* Собрать самому: `.\build-windows.ps1` (см. README).

**Продвинутый вариант: свой `.bat` с фиксацией частот и лимитом мощности через `nvidia-smi`.** Если нужен ещё и **лимит
мощности** (встроенное управление частотами его не задаёт) или свой цикл перезапуска и файл лога — сделайте `.bat` как ниже и
запускайте его **от администратора** (`nvidia-smi -lgc/-lmc/-pl` и `--auto-uv` требуют прав администратора):

```bat
@echo off
rem Фиксируем частоты и лимит мощности через драйвер (администратор; сбрасывается при перезагрузке). Значения — раздел 7.
nvidia-smi -lgc 1665
nvidia-smi -lmc 810
nvidia-smi -pl 165
cd /d C:\Mining\freeforgempear-1.5.1-windows-x64
:loop
powershell -NoProfile -Command "& { .\freeforgempear.exe --pool pearl-eu2.luckypool.io:3360 --wallet ВАШ_АДРЕС_PRL --worker rig1 --auto-uv --uv-min 135 --uv-max 360 | Tee-Object -Append -FilePath C:\Mining\ffgp.log }"
timeout /t 10
goto loop
```

Замените `ВАШ_АДРЕС_PRL` на свой адрес `prl1...` (для объединённого майнинга PRL+NOCK на LuckyPool допишите `+ВАШ_АДРЕС_NOCK`).
`-lgc` фиксирует частоту ядра, `-lmc` — памяти, `-pl` — лимит мощности в ваттах; `timeout /t 10` + `goto loop` перезапускают
майнер через 10 с после любого выхода, а `Tee-Object` пишет лог, не скрывая вывод. Снять фиксацию без перезагрузки:
`nvidia-smi -rgc` и `nvidia-smi -rmc`. (Штатный `start.bat` делает фиксацию частот и цикл перезапуска за вас через `LOCK_CCLOCK` /
`LOCK_MCLOCK` и `MOFFSET`; свой `.bat` нужен только ради `-pl` или собственного лога.)
С 1.5.1 такой файл лежит в Windows-архиве готовым: **`start-nvidia-smi.bat`** (фиксация частот, лимит мощности, диапазон
андервольта, цикл перезапуска и лог `logsfgp.log` в папке майнера; значения в нём — для RTX 3070, поменяйте под свою карту).

### 6. Пулы

По умолчанию **LuckyPool**, `pearl-eu2.luckypool.io:3360` (stratum по TCP). Хосты и порты ниже взяты из конфигурации самого пула
(`https://pearl.luckypool.io/api/stats`, `config.stratum` и `config.ports`); выбирайте ближайший регион:

| Регион | Хост |
|---|---|
| Европа | `pearl-eu2.luckypool.io` (по умолчанию), `pearl-eu1.luckypool.io`, `pearl-pl.luckypool.io` (Польша), `pearl-tr.luckypool.io` (Турция) |
| Россия | `pearl-ru.luckypool.io` |
| США | `pearl-us-east.luckypool.io`, `pearl-us-west.luckypool.io`, `pearl-us-central.luckypool.io`, `pearl-us-ord.luckypool.io` |
| Южная Америка | `pearl-br.luckypool.io` (Бразилия) |
| Канада | `pearl-ca1.luckypool.io`, `pearl-ca2.luckypool.io` |
| Азия | `pearl-sg1.luckypool.io`, `pearl-sg2.luckypool.io` (Сингапур), `pearl-id.luckypool.io` (Индонезия), `pearl-hk.luckypool.io` (Гонконг), `pearl-in.luckypool.io` (Индия), `pearl-jp.luckypool.io` (Япония), `pearl-au.luckypool.io` (Австралия) |

| Порт | Сложность шары (vardiff) |
|---|---|
| `3360` | 2 000 000 (по умолчанию) |
| `3361` | 4 000 000 |
| `3362` | 8 000 000 |

Пример: `--pool pearl-pl.luckypool.io:3360`. В качестве кошелька указывайте обычный адрес `prl1...`, имя воркера передаётся отдельно.
Подойдёт любой другой stratum-пул Pearl с тем же JSON-RPC; комиссия (раздел 10) тогда майнится на этом пуле.

**Merge-майнинг NOCK (только LuckyPool).** LuckyPool за ту же работу Pearl платит ещё и Nockchain (NOCK), без потери хэшрейта:
укажите кошелёк в виде `АДРЕС_PRL+АДРЕС_NOCK`, например `--wallet prl1...+ВАШ_АДРЕС_NOCK`. Статистика NOCK — на
`https://nock.luckypool.io` по вашему адресу NOCK. В батниках Windows впишите тот же `prl1...+NOCK...` в `WALLET`; в HiveOS —
в адрес кошелька PRL. Другие пулы (Kryptex) это не поддерживают: с 1.4.3 майнер сам отбрасывает там часть `+NOCK` (в логе
`PRL+NOCK merge mining works on LuckyPool only`) и майнит PRL на адрес `prl1...`.

**Kryptex** (проверено в 1.4.1): `--pool prl-eu.kryptex.network:7048` (также `prl`, `prl-us`, `prl-br`, `prl-sg`, `prl-hk`, `prl-ru`,
`prl-ae` `.kryptex.network`; порт 7048 — обычный TCP). Кошелёк — ваш адрес `prl1...`. Kryptex берёт имя воркера из логина, поэтому с 1.4.3
майнер отправляет туда `prl1....ВОРКЕР` (до 1.4.3 Kryptex показывал любой риг как `worker`).
Крупным ригам на Kryptex (больше ~0.5 PH/s на воркер) с 1.4.5 можно задать свою сложность шары: `--password d=ЗНАЧЕНИЕ`
(HiveOS — Extra config arguments; Windows — `PASSWORD` в `.bat`), `ЗНАЧЕНИЕ = хэшрейт (H/s) × секунд на шару / 4294967296`;
по умолчанию 2097152 (воркер на 100 TH/s находит шару примерно раз в 90 с).

**Сжатие доказательств.** Доказательства отправляются сжатыми gzip (около 5 КБ вместо примерно 270 КБ: меньше трафика, быстрее
отправка, меньше устаревших шар), если пул согласился на это при входе: с 1.4.4 майнер следует протоколу Pearl stratum gzip (отправляет
`"type":"v2"` в `mining.authorize`; Kryptex и LuckyPool отвечают `"type":"v2"`, в логе `pool confirmed gzip proofs`). Пулу, который этого
не подтвердил, доказательства идут несжатыми. `FFGP_PROOF_GZIP=on` / `off` задаёт выбор принудительно.
**На Kryptex версии 1.4.1–1.4.3** отправляли сжатые доказательства без этого согласования: Kryptex отвечал «принято», но шары
не засчитывал. Обновитесь до 1.4.4 или новее.

### 7. Разгон (лок ядра, память, V/F offset)

Ядро работает из L2-кэша и почти не нагружает видеопамять, поэтому частоту памяти можно опустить до минимума.
Важны три настройки: **зафиксированная частота ядра**, **низкая частота памяти** и **V/F offset** (андервольт: та же
частота при меньшем напряжении; `--auto-uv` подберёт его сам, раздел 12).

**Замер FreeForGemPear, RTX 3070:** ядро `1560` МГц (лок), память `810` МГц, лимит мощности `120` Вт →
**~70 TH/s при ~105–107 Вт**; `--auto-uv` затем находит +195..240 (≈ 86–107 Вт).

**Стартовые значения по картам.** Строки *опорные* — опубликованные результаты настройки этих карт; строки *оценка* выведены
по картам того же поколения и не замерены. Все карты разные: свой offset пусть подберёт `--auto-uv`.

| Карта | Лок ядра, МГц | Память | V/F offset, МГц | Источник |
|---|---|---|---|---|
| RTX 3060 Ti, RTX 3070, RTX 3070 Laptop, RTX 3080 Ti | 1560–1650 | лок `810` | +200..255 | опорные (3070 — наш замер) |
| RTX 3060, RTX 3070 Ti, RTX 3080, RTX 3090 | 1560–1650 | лок `810` | +200..255 | оценка |
| CMP 70HX, CMP 90HX (Ampere) | 1650 | offset `-2000` (`--moffset`) | +255..300 | опорные, с FreeForGemPear не проверялись |
| RTX 4070 Ti, RTX 4090 | 2445 | лок `810` | +315..345 | опорные |
| RTX 4060, 4060 Ti, 4070, 4070 Super, 4070 Ti Super, 4080, 4080 Super | 1900–2445 | лок `810` | +300..345 | оценка |
| RTX 5070 Ti | 2445 | лок `810` | +480 | опорные |
| RTX 5070 | 2400 | лок `810` | проверки проходят до +390, выше — падение | отчёт пользователя, FreeForGemPear 1.5.1, Windows, драйвер 617.14 |
| RTX 5060, 5060 Ti, 5080, 5090 | 1900–2500 | лок `810` | +390..480 | оценка |

RTX 40 и 50 работают на гораздо более высоком локе ядра (**1900–2500 МГц**) и держат больший offset, чем RTX 30; поэтому
auto-uv начинает поиск на них с +200 (RTX 30 — с +120). Turing / CMP 30HX–50HX (sm_75) и CMP 170HX / A100 (sm_80) не поддерживаются.

* HiveOS: вкладка Overclock → частота ядра (абсолютная), частота памяти (абсолютная), лимит мощности; V/F offset — через `--auto-uv`.
* Linux: `nvidia-smi -lgc 1560,1560`, `nvidia-smi -lmc 810,810`, `nvidia-smi -pl 120` (root) или `--lock-cclock` / `--lock-mclock`.
* Windows: `LOCK_CCLOCK` / `LOCK_MCLOCK` / `MOFFSET` в `start.bat` (от администратора), `start-nvidia-smi.bat` — если нужен
  лимит мощности, или MSI Afterburner. Готовые комбинации по картам — раздел 12, *Windows: готовые комбинации*.

Если пул отклоняет шары или в логе есть `bad`-попадания, снизьте частоту ядра или offset.

### 8. Как читать лог

| Строка | Значение |
|---|---|
| `FreeForGemPear 1.4.9 \| pool ... \| worker ... \| N GPU(s)` | стартовая строка |
| `[pearlgpu] search kernel: SASS-patched cubin (...)` | загружено настроенное ядро. `embedded search kernel could not be loaded (причина)` = майнер останавливается с ошибкой (драйвер старше 525 или неподдерживаемая карта) |
| `new job ... prepared in ...` | готово новое задание пула |
| `[ 120s] GPU0 ... \| total X TH/s \| hits ... \| user sub S acc A rej R \| fee sub ... acc ... rej ...` | каждые 10 с: хэшрейт по картам и общий; отправленные / принятые / отклонённые шары ваши и комиссии; `[DEVFEE]` пока идёт слот комиссии |
| `[DISCONNECTED]` | соединение с пулом потеряно; майнер переподключится сам |
| `devfee: ...` | слоты комиссии (раздел 10) |
| `!! ...` | предупреждение или ошибка (остановился GPU-воркер, hit не отправлен) |

### 9. Проверка загрузки (SHA-256)

Сравните результат с хешем на странице релиза.

* Linux: `sha256sum freeforgempear-1.5.1-linux-x86_64.tar.gz freeforgempear-1.5.1.tar.gz`
* Windows: `certutil -hashfile freeforgempear-1.5.1-windows-x64.zip SHA256` или
  `Get-FileHash freeforgempear-1.5.1-windows-x64.zip -Algorithm SHA256`

### 10. Комиссия — 1%

Один 60-секундный слот в каждом 100-минутном цикле, в случайный момент цикла, на **том же пуле, который вы указали**, под отдельным
логином (воркер `ffgp-devfee`). В логе: `devfee: next slot in N min`, `devfee: mining 60s for FreeForGemPear dev (1%)`,
`devfee: back to user`. Первый слот приходится на случайный момент первых 100 минут. Если логин комиссии не обслуживается, слот
пропускается; ваш майнинг не блокируется. На LuckyPool логин комиссии с 1.4.2 майнит заодно и NOCK разработчика
(`prl1...+NOCK`); если пул не примет такой логин, майнер вернётся к обычному кошельку PRL. Время комиссии остаётся 1%. Комиссия — условие лицензии: убирать, уменьшать или перенаправлять её нельзя
(см. [LICENSE](LICENSE), пункт 3).

### 11. Проблемы и решения

| Симптом | Решение |
|---|---|
| Проверить, что всё работает | `freeforgempear --self-test` (Windows: `freeforgempear.exe --self-test`) должен закончиться строкой `== self-test PASSED ==` |
| При старте `embedded search kernel could not be loaded` | обновите драйвер NVIDIA (525+; **для RTX 50xx нужен 570+**) и проверьте, что карта Ampere (RTX 30xx) или новее |
| Высокая загрузка CPU | по умолчанию ожидание GPU спит (заметно меньше 10% ядра на карту). `PEARL_SPIN=1` включает активное ожидание (ядро CPU на карту): убедитесь, что она не задана |
| Ошибки `pg_init(...)` или нехватка памяти | видеокарту занимает другая программа (игры, локальные нейросети). Закройте её или выберите карты через `--devices` |
| Долго `[DISCONNECTED]` | проверьте хост/порт пула и брандмауэр (исходящий TCP); попробуйте другой регион из раздела 6 |
| Много отклонённых шаров | снизьте частоту ядра (раздел 7), проверьте охлаждение |
| HiveOS пишет `Already installed` и ничего не меняется | Installation URL должен указывать на новую версию |
| Windows: окно сразу закрывается | запустите `start.bat` из `cmd`, чтобы увидеть сообщение; в пути не должно быть пробелов и русских букв |
| Windows: `.exe` блокирует Defender / SmartScreen | *Подробнее → Выполнить в любом случае* или добавьте папку в исключения; сверьте SHA-256 |

Переменные окружения (задаются до запуска майнера): `PEARL_SPIN=1` (активное ожидание вместо сна).
`FFGP_M=N` задаёт число строк A за попытку (кратно 128). По умолчанию `m` равен 47104, и майнер сам подбирает `m` для каждой GPU по числу её SM (`m = 128 * SM * r`), поэтому на риге с разным числом SM каждая карта работает со своим `m` — по одному заданию на группу SM (с 1.4.7). Задавайте `FFGP_M` только для экспериментов: он задаёт один `m` на всех устройствах, а чем больше `m`, тем немного выше производительность, но тем больше расход оперативной памяти хоста.
В Windows: `set PEARL_SPIN=1` в окне `cmd`, затем запустите `start.bat` из этого же окна.

### 12. Андервольт / `--auto-uv` (необязательно, новое в 1.4.0)

**По умолчанию выключено.** С `--auto-uv` майнер один раз для каждой карты подбирает пониженное напряжение ядра
(*V/F offset* видеокарты), при котором карта всё ещё считает без ошибок, использует его при майнинге и возвращает ваши
настройки при выходе. Меньше напряжение — меньше ватт при том же хешрейте: частота остаётся той, что вы зафиксировали (раздел 7).

**Что делает**
1. Читает V/F offset, который стоит сейчас (разгон в HiveOS, ваш скрипт и т. п.), и записывает его в файл восстановления
   *до* любых изменений.
2. Прогревает карту 3 минуты, затем поднимает offset шагами по **15 МГц**. Каждый шаг около 30 с проверяется в отдельном
   процессе: одна и та же задача считается тысячи раз, каждый тайл каждого результата сравнивается побитно, сравниваются
   счётчики попаданий обычных майнинговых попыток, а найденные результаты проверяются на CPU официальным верификатором.
   У предела бывают неверные результаты без всякой ошибки драйвера, поэтому проверка идёт и на GPU, и на CPU.
3. Первое расхождение, ошибка CUDA, Xid или зависание останавливают подъём. Итог — **последний успешный шаг минус один шаг
   (запас 15 МГц)**, подтверждённый более длинной проверкой (около 2 минут); если подтверждение не прошло, берётся шагом ниже.
   Выше +495 не поднимается (`--uv-max`, максимум +600; до 1.5.1 было +300 с жёстким потолком +400). Карты RTX 40/50
   часто держат +450..480.
4. Результат сохраняется в `uv.json` с ключом: UUID карты, версия драйвера, сборка ядра. При следующих запусках он
   применяется после быстрой проверки (около 30 с). Если изменились драйвер, сборка ядра или ваш начальный offset,
   карта подбирается заново.
5. Во время майнинга: каждые 6 часов (`--uv-reval-hours`) настройка перепроверяется (майнинг на этой карте приостанавливается
   примерно на 30 с). Если найденный результат не прошёл проверку на CPU или пошла серия отклонённых шар, offset
   снижается на шаг (но не ниже вашего значения) и сохраняется. Если offset изменил кто-то другой (например, HiveOS заново
   применил разгон), auto-uv отступает на этой карте и не воюет.
6. При выходе (Ctrl+C, SIGTERM, обычное завершение) ваш offset возвращается. Если майнер был убит (`kill -9`, сбой), то при
   следующем запуске ваш offset сначала восстанавливается из файла `uv-restore.json`. Перезагрузка в любом случае сбрасывает
   offset в драйвере.

Меняется только V/F offset: зафиксированные частоты, лимит мощности и память не затрагиваются. Затрагиваются только карты,
на которых майнер работает. Номера устройств NVML отличаются от номеров CUDA, поэтому каждая карта сопоставляется по
PCI-адресу (шине) и UUID; в логе у каждого изменения указан адрес шины.

**Требования.** Драйвер NVIDIA (NVML подгружается во время работы, ставить ничего не нужно) и **root** в Linux / HiveOS
(там майнер запускается от root) или **администратор** в Windows. Без NVML или прав майнер пишет причину в лог и
майнит как обычно. **Время:** первый подбор занимает около **15 минут на карту** (10-25 в зависимости от числа шагов; карты рига подбираются параллельно,
майнинг начинается после окончания). Последующие запуски добавляют около 30 с.

**Windows: по шагам.** Проверено на Windows 11, GeForce RTX 3070, драйвер 581.80: драйвер даёт майнеру читать и менять
offset (в логе `allowed -1000..1000`), подбор нашёл +240 вместо +225 (98.4 Вт → 86.5 Вт на той же частоте), и защита тоже
сработала (найденный результат не прошёл проверку на CPU → обратно на +225, auto-uv на паузе 24 ч).
1. **Администратор.** Правой кнопкой по `.bat` → *Запуск от имени администратора*. Для автозапуска — задача в Планировщике
   с галкой *Выполнить с наивысшими правами*. Без прав в логе будет сказано, что offset поменять нельзя, и майнер просто майнит.
2. **Зафиксируйте частоту ядра** (раздел 7): `set "LOCK_CCLOCK=1560"` в `start.bat` (одно значение или список через запятую
   по картам) или `nvidia-smi -lgc` в `start-nvidia-smi.bat`. С зафиксированной частотой больший offset = меньшее напряжение
   на той же частоте; без фиксации карта может вместо этого буститься выше, и экономия меньше.
3. **Включите**, любой из вариантов:
   * `start.bat` (или любой `start-luckypool-*` / `start-kryptex-*`): уберите `rem` перед `set "AUTO_UV=1"`. Верхний предел —
     `set "UV_MAX="` (пусто = +495; RTX 40/50 часто держат +450..480, RTX 30 обычно +200..360).
   * `start-nvidia-smi.bat`: `AUTO_UV=1`, диапазон в `UV_MIN` / `UV_MAX`, плюс лимит мощности и фиксация частот через `nvidia-smi`.
   * своя командная строка: `freeforgempear.exe --pool ... --wallet ... --auto-uv --uv-max 480`.
4. **Точка старта.** Подбор начинает с offset, который сейчас стоит на карте, и ниже него не опускается. Смещение *Core Clock*
   в MSI Afterburner (и похожих программах) — это тот же самый offset: выставленное там значение станет точкой старта и
   вернётся при выходе. Не давайте Afterburner заново применять профиль во время майнинга: auto-uv увидит, что offset
   поменял кто-то другой, и отступит на этой карте. Если offset не задан, подбор стартует с +120, на RTX 40/50 — с +200 (или с `--uv-min` / `UV_MIN`).
5. **Подождите.** При первом запуске подбор идёт около 15 минут на карту (все карты параллельно), майнинг начинается после
   него. Следующие запуски применяют сохранённое значение после проверки ~30 с.
6. **Файлы** лежат в папке майнера: `uv.json` (результаты) и `uv-restore.json` (ваш исходный offset). Папка должна быть
   доступна на запись, поэтому не кладите майнер в `C:\Program Files`. Подобрать заново: закройте майнер, удалите `uv.json`
   (или запустите в этой папке `freeforgempear.exe --uv-reset`) и запустите снова. Новый драйвер или новое ядро майнера
   запускают подбор заново сами.
7. **Результат смотрите так:** `findstr auto-uv logs\*.log` (у `start-nvidia-smi.bat` — `logs\ffgp.log`). Пример с той RTX 3070:
```
auto-uv: GPU0 (bus 04, RTX 3070): CUDA device 0 -> NVML index 0, PCI 0000:04:00.0, ..., current offset +225 (allowed -1000..1000)
auto-uv: GPU0 (bus 04, RTX 3070) step +255: ok (6560 reruns, cpu ok 6), 94.0 W, 1560 MHz, 58 C
auto-uv: GPU0 (bus 04, RTX 3070) step +270: FAILED (rerun_diff: 256 transcript word(s) differ between reruns of seed 3 ...)
auto-uv: GPU0 (bus 04, RTX 3070) offset +225 -> +240, 98.4 W -> 86.5 W, saved
```
   * `auto-uv paused for this GPU for 24 h` — на найденном offset у карты была ошибка; она майнит на вашем offset, подбор
     повторится через 24 ч (или после `--uv-reset`).
   * `start offset ... leaves no room below the cap` — ваш offset уже равен `UV_MAX` или выше; поднимите `UV_MAX`.
   * `no safe offset above ... was found` — карта не держит больше вашего значения; ничего не меняется.
8. **Если ПК завис или сбросился драйвер** (чёрный экран, «видеодрайвер перестал отвечать»): перезагрузитесь и запустите
   майнер снова — он сначала вернёт ваш offset из `uv-restore.json`. Если сам майнер упал на шаге подбора (в окне
   `Miner exited (code -1073741819)`), следующий запуск считает этот шаг провальным и подбирает только ниже него (с 1.5.2;
   раньше подбор начинался заново и падал на том же шаге). Если карта сбоит снова и снова, уменьшите `UV_MAX` или
   не включайте `AUTO_UV`.

**Windows: готовые комбинации.** Правой кнопкой по `start.bat` → *Изменить*, уберите `rem` перед `set "AUTO_UV=1"`,
заполните поля ниже, сохраните и запускайте **от администратора**. Пустые поля — значения по умолчанию. Значения — стартовые,
из таблицы в разделе 7.
```bat
rem --- RTX 30 (3060 Ti / 3070 / 3080 ...) ---
set "AUTO_UV=1"
set "LOCK_CCLOCK=1560"
set "LOCK_MCLOCK=810"
set "UV_MIN=150"
set "UV_MAX=360"

rem --- RTX 40 (4060 ... 4090) ---
set "AUTO_UV=1"
set "LOCK_CCLOCK=2445"
set "LOCK_MCLOCK=810"
set "UV_MIN=200"
set "UV_MAX=450"

rem --- RTX 50 (5060 ... 5090): старт выше, подбор короче ---
set "AUTO_UV=1"
set "LOCK_CCLOCK=2445"
set "LOCK_MCLOCK=810"
set "UV_MIN=300"
set "UV_MAX=495"
```
* **Смешанный риг:** `LOCK_CCLOCK` / `LOCK_MCLOCK` принимают по значению на карту в порядке `DEVICES`, например
  `set "LOCK_CCLOCK=1560,2445"`. `UV_MIN` / `UV_MAX` общие для всех карт: оставьте пустыми (RTX 40/50 сами стартуют с +200)
  или возьмите значения младшей карты.
* **Быстрее подбор:** `set "UV_STEP=30"` вдвое сокращает число шагов (результат грубее). На RTX 50 больше всего времени
  экономит высокий `UV_MIN` (около +300): карта его всё равно держит, и поиск начинается сразу оттуда. Не завышайте: если
  карта не проходит проверку уже на `UV_MIN`, подбора не будет и останется ваш offset (это видно в логе; тогда уменьшите `UV_MIN`).
* **Offset уже известен:** `set "UV_OFFSET=480"` (а `AUTO_UV` не включать) — применится после одной быстрой проверки, без поиска.
* **Нужен ещё лимит мощности:** используйте `start-nvidia-smi.bat` (`CCLOCK` / `MCLOCK` / `PLIMIT`, `UV_MIN` / `UV_MAX` / `UV_STEP`).
* **Подобрать заново после смены диапазона:** на один запуск `set "EXTRA=--uv-reset"`, потом снова очистите.

**Риск.** При слишком агрессивном андервольте возможен сброс драйвера или зависание (подбор идёт только по тем значениям, которые
только что проверены, и с запасом, но кристаллы, температура и драйверы разные). После зависания перезагрузитесь: при следующем
запуске ваш offset будет восстановлен первым делом. Используйте на свой риск, как и любой разгон.

**Включение**
* HiveOS: добавьте `--auto-uv` в *Extra config arguments* полётного листа (подробнее в HIVEOS.md).
* Linux: `sudo ./freeforgempear --pool ... --wallet ... --auto-uv`
* Windows: правой кнопкой по `start.bat` → *Изменить*, уберите `rem` перед `set "AUTO_UV=1"`; запускайте `start.bat` **от администратора**.

| Параметр | Назначение |
|---|---|
| `--auto-uv` | подобрать, применить и сопровождать андервольт (как описано выше) |
| `--uv-offset N` | применить фиксированный offset N МГц после быстрой проверки, без подбора (если проверка не прошла, остаётся ваш offset) |
| `--uv-reset` | забыть сохранённые результаты; вместе с `--auto-uv` карты подбираются заново; без кошелька майнер просто завершается |
| `--uv-max N` | наибольший offset, который пробует подбор (по умолчанию 495, максимум 600; в Windows: `set "UV_MAX=480"` в `start.bat`) |
| `--uv-min N` | offset, с которого начинается подбор и ниже которого не опускается (по умолчанию offset из HiveOS/драйвера, иначе 120) |
| `--uv-step N` | шаг подбора в МГц (по умолчанию 15) |
| `--uv-keep-last` | после неудачного шага оставить последний удачный шаг (по умолчанию — последний удачный минус один шаг) |
| `--uv-reval-hours H` | интервал перепроверки во время майнинга (по умолчанию 6, `0` — не перепроверять) |
| `--uv-dir DIR` | где лежат `uv.json` и `uv-restore.json` (также `FFGP_STATE_DIR`) |
| `--auto-uv-dry-run` | только пишет в лог, какие карты были бы изменены (устройство CUDA → устройство NVML, шина); ничего не меняет |

**Управление частотами GPU (по желанию, через NVML; нужен root / администратор, работает в Windows — с 1.4.9).** `--lock-cclock N` фиксирует частоту ядра на N МГц (как `nvidia-smi -lgc N,N`), `--lock-mclock N` фиксирует частоту памяти на N МГц (как `-lmc`), `--moffset N` задаёт смещение частоты памяти в МГц, может быть отрицательным (нужен драйвер 555+). Каждая опция принимает одно значение для всех GPU или список через запятую в порядке `--devices` (по одному на карту). Настройки применяются при старте и снимаются при выходе (Ctrl-C / SIGTERM / сбой). В Windows задайте `LOCK_CCLOCK`, `LOCK_MCLOCK`, `MOFFSET` в `.bat`-файле. Пример: `--lock-cclock 1500 --lock-mclock 810 --moffset -200`.

Файлы по умолчанию лежат рядом с программой (Windows — в папке майнера), в HiveOS — `/hive-config/freeforgempear`
(переживает обновления майнера и перезагрузки). **Сброс:** `freeforgempear --uv-reset` (или удалить `uv.json`).

**Строки лога** (все начинаются с `auto-uv:`, так что `grep auto-uv` показывает всю историю):
```
auto-uv: GPU0 (bus 0d, RTX 3070): CUDA device 0 -> NVML index 1, PCI 0000:0D:00.0, GPU-0d06..., current offset +150
auto-uv: GPU0 (bus 0d, RTX 3070) step +195: ok (1952 reruns, cpu ok 6), 110.4 W, 1560 MHz, 53 C
auto-uv: GPU0 (bus 0d, RTX 3070) step +225: FAILED (rerun_diff: ...)
auto-uv: GPU0 (bus 0d, RTX 3070) offset +150 -> +195, 116.6 W -> 106.6 W, saved
auto-uv: GPU0 (bus 0d, RTX 3070) offset +195 -> +150 restored
```

**Ориентиры, RTX 3070, ядро 1560 МГц, память 810 МГц** (~70 TH/s при любом offset, хешрейт не меняется):
около 116 Вт при +150, 113 Вт при +180, 110 Вт при +195 и 107–108 Вт при +210. На нашей тестовой карте встроенная проверка
нашла тихие ошибки вычислений начиная с +225, хотя более короткий ручной тест проходил даже +255, поэтому подбор остановился на
+195…+210. Тихие ошибки не дают ни Xid, ни отклонённых шар, но теряют верные шары: offset, который «выглядит стабильным», может
приносить меньше. Предел у каждого кристалла свой и зависит от температуры: ждите порядка 5–10 Вт на карту по сравнению со
стоковым offset, проверяйте своё значение через `--uv-offset N` и не копируйте чужие числа.

**Ручная настройка (без `--auto-uv`)** — для тех, кто предпочитает вкладку OC в HiveOS или Afterburner:
1. Зафиксируйте частоту ядра и снизьте частоту памяти, как в разделе 7; дайте карте прогреться несколько минут.
2. Поднимите **offset** ядра (V/F offset: вкладка Overclock в HiveOS, Afterburner или `--uv-offset N`) на 15 МГц.
3. Проработайте не менее 30 минут и следите: `bad` должно оставаться 0, `rej` не должно расти, в логе не должно быть `DEAD`, а в
   `dmesg` / сообщениях HiveOS — `Xid` (13, 31, 43, 79 ...). `freeforgempear --uv-offset N` применяет N, выполняет ту же
   проверку, что и `--auto-uv`, и майнит с ним — это быстрый способ проверить кандидата.
4. Повторяйте с шага 2 до первой проблемы, затем вернитесь на **два шага (30 МГц)** от значения, где она появилась, и оставьте его.
5. Проверьте заново после обновления драйвера, нового кулера или в жаркую погоду.

---

## 中文

### 1. 这是什么

FreeForGemPear 是面向 NVIDIA 显卡的 **Pearl (PRL)**（pearlhash，有用工作量证明）GPU 矿工，开发者抽水 **1%**。
它使用针对 Ampere 的自研 CUDA 内核：A′ 矩阵在芯片上生成，int8 张量核心 GEMM 在常驻内核中运行，jackpot 折叠和 BLAKE3
哈希都融合在其中。主循环在 SASS 级别手工调优。提交之前，主机会用官方 `zk-pow` 验证器构建并检查每个份额的证明。

| 显卡系列 | 状态 |
|---|---|
| RTX 30xx（Ampere，sm_86）：3060 / 3060 Ti / 3070 / 3070 Ti / 3080 / 3090 | **主要目标，已调优：RTX 3070 约 70 TH/s，约 107 W** |
| RTX 40xx（Ada，sm_89） | 运行 sm_86 代码，未调优 |
| RTX 50xx（Blackwell，sm_120） | 自 1.5.0 起支持，使用编译内核（未调优）；需要驱动 570+ |
| 更老的显卡、A100（sm_80）、AMD | 不支持 |

### 2. 系统要求

* RTX 30 系列 NVIDIA 显卡（RTX 40 未调优可运行，RTX 50 自 1.5.0 起支持），NVIDIA 驱动 **525 或更新**（RTX 50xx：**570+**）。
* HiveOS、Linux x86_64（glibc 2.31+，Ubuntu 20.04+）或 Windows 10/11 x64。不需要 CUDA toolkit、Python 或 WSL。
* PRL 钱包地址（`prl1...`）以及到 Pearl stratum 矿池的网络连接。
* 显存：内核从 L2 缓存工作，几乎不占用显存。

### 3. HiveOS 设置（分步说明）

FreeForGemPear 在 HiveOS 中作为 **Custom miner** 运行：无需 SSH，无需手动安装。

**安装链接 (1.4.9)：**

```
https://github.com/pepsykolya/FreeForGemPear/releases/download/v1.5.1/freeforgempear-1.5.1.tar.gz
```

1. **钱包。** 在 *Wallets* 中添加 PRL 钱包：币种 `PRL`，你的 `prl1...` 地址。
2. **新建 Flight sheet。**
   * **Coin：** `PRL`。
   * **Wallet：** 第 1 步的钱包。
   * **Pool：** *Configure in miner*。
   * **Miner：** *Custom*，然后点击 *Setup Miner Config*。
3. **矿工配置：**

   | 字段 | 值 |
   |---|---|
   | Miner name | `freeforgempear`（粘贴链接后自动填入） |
   | Installation URL | 上面的链接 |
   | Hash algorithm | `pearlhash`（或保持 `----`） |
   | Wallet and worker template | `%WAL%.%WORKER_NAME%` |
   | Pool URL | `pearl-eu2.luckypool.io:3360`，或 Kryptex `prl-eu.kryptex.network:7048`（第 6 节中的任一矿池） |
   | Pass | 留空 |
   | Extra config arguments | 可选，例如 `--devices 0,1` |

4. **应用。** *Apply changes*，*Create flight sheet*，然后应用到矿机。

矿工日志位于 `/var/log/miner/freeforgempear/freeforgempear.log`，也可在 HiveOS 矿工控制台查看。
仪表盘显示每块显卡和总算力、温度、风扇，以及已接受和被拒绝的份额。更新时把 Installation URL 改成新版本即可。
更详细的同一指南：[HIVEOS.md](HIVEOS.md)。

### 4. 不使用 HiveOS 的 Linux

```
tar xzf freeforgempear-1.5.1-linux-x86_64.tar.gz
cd freeforgempear-1.5.1
./freeforgempear --self-test        # 可选：将 GPU 内核与 CPU 参考实现比对
./freeforgempear --pool pearl-eu2.luckypool.io:3360 --wallet 你的PRL地址 --worker rig1     # LuckyPool
./freeforgempear --pool prl-eu.kryptex.network:7048 --wallet 你的PRL地址 --worker rig1       # Kryptex
```

| 选项 | 含义 |
|---|---|
| `--pool HOST:PORT` | stratum 矿池（默认 `pearl-eu2.luckypool.io:3360`） |
| `--wallet ADDRESS` | 你的 PRL 地址（必填） |
| `--worker NAME` | 矿工名（默认 `worker1`） |
| `--devices 0,1,...` | 使用的显卡（默认所有可见显卡；遵循 `CUDA_VISIBLE_DEVICES`） |
| `--minutes N` | N 分钟后停止（默认一直运行到 Ctrl-C / SIGTERM） |
| `--self-test` | 验证内核后退出 |

一个进程驱动所有显卡，并自动重连矿池。长期运行请放在 `systemd`、`screen` 或 `tmux` 下。

### 5. Windows 10/11

原生 Windows 版本：无需 WSL、无需安装 CUDA、无需 Python。只需要 NVIDIA 驱动。

1. 从发布页面下载 **`freeforgempear-1.5.1-windows-x64.zip`**：
   `https://github.com/pepsykolya/FreeForGemPear/releases/download/v1.5.1/freeforgempear-1.5.1-windows-x64.zip`
   并核对 SHA-256（第 9 节）。
2. 解压到**不含空格和非英文字符**的文件夹，例如 `C:\FreeForGemPear`。
3. 选择对应矿池的启动文件——每个都可直接运行，只需修改钱包：

   | 文件 | 矿池 |
   |---|---|
   | `start.bat`、`start-luckypool-eu.bat` | LuckyPool 欧洲 `pearl-eu2.luckypool.io:3360` |
   | `start-luckypool-ru.bat` / `-us.bat` / `-asia.bat` | LuckyPool 俄罗斯 / 美国东部 / 新加坡 |
   | `start-kryptex-eu.bat` | Kryptex 欧洲 `prl-eu.kryptex.network:7048` |
   | `start-kryptex-ru.bat` / `-us.bat` / `-asia.bat` | Kryptex 俄罗斯 / 北美 / 新加坡 |

   **重要：**所有文件中的 `WALLET` 预填的是**开发者的示例地址**。请替换为你自己的地址，否则你是在为开发者挖矿（启动时矿工会提示）。
   右键点击文件 → *编辑*，并设置：

| 设置项 | 值 |
|---|---|
| `WALLET` | 你的 PRL 地址（`prl1...`） |
| `POOL` | `HOST:PORT`，默认 `pearl-eu2.luckypool.io:3360`（第 6 节） |
| `WORKER` | 矿机名称，默认 = 计算机名 |
| `DEVICES` | 留空 = 所有 NVIDIA 显卡，或类似 `0,1` 的列表（编号来自 `nvidia-smi`） |

4. 双击所选的 **`.bat`** 文件。一个矿工进程驱动所有显卡；输出显示在窗口中并保存到
   `logs\miner-<日期>.log`（保留最近 10 个日志）。矿工退出或崩溃后会在 15 秒后自动重启。`Ctrl+C` 停止。

* **开机自启：** 把 `start.bat` 的快捷方式放进 `shell:startup`（Win+R，输入 `shell:startup`），或在任务计划程序中创建"登录时"运行 `start.bat` 的任务。
* **超频：** 使用 MSI Afterburner（在曲线编辑器中锁定核心频率，降低显存频率），数值见第 7 节，**或**使用矿工内置的频率控制（下一条）。笔记本：接上充电器并选择最高性能模式。
* **内置频率控制（1.4.9 新增）：** 在 `.bat` 文件中设置 `LOCK_CCLOCK`、`LOCK_MCLOCK` 和/或 `MOFFSET`（第 12 节），例如 `set "LOCK_CCLOCK=1560"` 和 `set "LOCK_MCLOCK=810"`。矿工在启动时锁定频率，退出时恢复。
* **降压（可选）：**删除 `start.bat` 中 `set "AUTO_UV=1"` 前面的 `rem`，并以管理员身份运行；见第 12 节 *Windows 分步说明*。
* **任何 GPU 调节都需以管理员身份运行。** `AUTO_UV`、`LOCK_CCLOCK`、`LOCK_MCLOCK` 和 `MOFFSET` 通过驱动修改 GPU，需要管理员权限：右键 `.bat` →**以管理员身份运行**（开机自启时，在计划任务中勾选*使用最高权限运行*）。否则矿工仍会运行，但使用当前频率并在日志中给出警告。纯挖矿（仅钱包/矿池）**不**需要管理员。
* Defender / SmartScreen 可能拦截新的未签名 `.exe`：点击"更多信息 → 仍要运行"，或将该文件夹加入排除项。
* 不使用 `start.bat`：`freeforgempear.exe --pool pearl-eu2.luckypool.io:3360 --wallet 你的PRL地址 --worker rig1`
* 自行编译：`.\build-windows.ps1`（见 README）。

**进阶：使用 `nvidia-smi` 锁频 + 功耗限制的自定义 `.bat`。** 如果你还需要**功耗限制**（内置频率控制不设置功耗限制），或想要自己的重启循环和日志文件，可以写一个如下的 `.bat`，并**以管理员身份**运行（`nvidia-smi -lgc/-lmc/-pl` 和 `--auto-uv` 都需要管理员权限）：

```bat
@echo off
rem 通过驱动锁定频率和功耗限制（需管理员；重启后失效）。数值见第 7 节。
nvidia-smi -lgc 1665
nvidia-smi -lmc 810
nvidia-smi -pl 165
cd /d C:\Mining\freeforgempear-1.5.1-windows-x64
:loop
powershell -NoProfile -Command "& { .\freeforgempear.exe --pool pearl-eu2.luckypool.io:3360 --wallet 你的PRL地址 --worker rig1 --auto-uv --uv-min 135 --uv-max 360 | Tee-Object -Append -FilePath C:\Mining\ffgp.log }"
timeout /t 10
goto loop
```

把 `你的PRL地址` 换成你的 `prl1...` 地址（在 LuckyPool 上进行 PRL+NOCK 合并挖矿时追加 `+你的NOCK地址`）。`-lgc` 锁定核心频率，`-lmc` 锁定显存频率，`-pl` 设置功耗限制（瓦）；`timeout /t 10` + `goto loop` 在任何退出后 10 秒重启矿工，`Tee-Object` 在显示输出的同时写入日志。不重启即可解除锁定：`nvidia-smi -rgc` 和 `nvidia-smi -rmc`。（自带的 `start.bat` 已通过 `LOCK_CCLOCK` / `LOCK_MCLOCK` 和 `MOFFSET` 为你完成锁频和重启循环；仅当你需要 `-pl` 或自定义日志时才用这种自定义 `.bat`。）
自 1.5.1 起 Windows 压缩包自带此文件，可直接编辑：**`start-nvidia-smi.bat`**（锁频、功耗限制、降压范围、重启循环，日志写入矿工目录下的 `logsfgp.log`；其中的数值针对 RTX 3070，请按你的显卡修改）。

### 6. 矿池

默认 **LuckyPool**，`pearl-eu2.luckypool.io:3360`（TCP stratum）。下面的主机和端口来自矿池自身配置
（`https://pearl.luckypool.io/api/stats` 中的 `config.stratum` 和 `config.ports`）；请选择离你最近的区域：

| 区域 | 主机 |
|---|---|
| 欧洲 | `pearl-eu2.luckypool.io`（默认）、`pearl-eu1.luckypool.io`、`pearl-pl.luckypool.io`（波兰）、`pearl-tr.luckypool.io`（土耳其） |
| 俄罗斯 | `pearl-ru.luckypool.io` |
| 美国 | `pearl-us-east.luckypool.io`、`pearl-us-west.luckypool.io`、`pearl-us-central.luckypool.io`、`pearl-us-ord.luckypool.io` |
| 南美 | `pearl-br.luckypool.io`（巴西） |
| 加拿大 | `pearl-ca1.luckypool.io`、`pearl-ca2.luckypool.io` |
| 亚洲 | `pearl-sg1.luckypool.io`、`pearl-sg2.luckypool.io`（新加坡）、`pearl-id.luckypool.io`（印尼）、`pearl-hk.luckypool.io`（香港）、`pearl-in.luckypool.io`（印度）、`pearl-jp.luckypool.io`（日本）、`pearl-au.luckypool.io`（澳大利亚） |

| 端口 | 份额难度（vardiff） |
|---|---|
| `3360` | 2 000 000（默认） |
| `3361` | 4 000 000 |
| `3362` | 8 000 000 |

示例：`--pool pearl-pl.luckypool.io:3360`。钱包填写普通的 `prl1...` 地址，矿工名单独发送。
也可使用其他使用相同 JSON-RPC stratum 的 Pearl 矿池；此时开发者抽水（第 10 节）在该矿池上进行。

**NOCK 合并挖矿（仅 LuckyPool）。** LuckyPool 会为同一份 Pearl 算力额外支付 Nockchain（NOCK），不损失算力：钱包写成
`PRL地址+NOCK地址`，例如 `--wallet prl1...+你的NOCK地址`。NOCK 统计见 `https://nock.luckypool.io`（按你的 NOCK 地址查询）。
Windows `.bat` 中把同样的 `prl1...+NOCK...` 填入 `WALLET`；HiveOS 中填入 PRL 钱包地址。其他矿池（Kryptex）不支持：自 1.4.3 起矿工会在这些矿池上自动去掉 `+NOCK` 部分（日志：`PRL+NOCK merge mining works on LuckyPool only`），只向 `prl1...` 挖 PRL。

**Kryptex**（1.4.1 已验证）：`--pool prl-eu.kryptex.network:7048`（另有 `prl`、`prl-us`、`prl-br`、`prl-sg`、`prl-hk`、`prl-ru`、
`prl-ae` `.kryptex.network`；7048 端口为普通 TCP）。钱包填写你的 `prl1...` 地址。Kryptex 从登录名中读取矿工名，因此自 1.4.3 起矿工在该矿池发送 `prl1....矿工名`（1.4.3 之前 Kryptex 把所有矿机显示为 `worker`）。
大型矿机在 Kryptex 上（每个矿工超过约 0.5 PH/s）自 1.4.5 起可设置自定义份额难度：`--password d=数值`（HiveOS：Extra config arguments；Windows：`.bat` 中的 `PASSWORD`），`数值 = 算力 (H/s) × 每个份额秒数 / 4294967296`；默认 2097152（100 TH/s 的矿工约每 90 秒一个份额）。

**证明压缩。** 当矿池在登录时同意后，证明以 gzip 压缩发送（约 5 KB，而不是约 270 KB：流量更少、提交更快、过期份额更少）：自 1.4.4 起矿工遵循
Pearl stratum gzip 协议（在 `mining.authorize` 中发送 `"type":"v2"`；Kryptex 和 LuckyPool 回复 `"type":"v2"`，日志 `pool confirmed gzip proofs`）。
未确认的矿池收到未压缩的证明。`FFGP_PROOF_GZIP=on` / `off` 可强制指定。
**在 Kryptex 上使用 1.4.1–1.4.3 的用户：**这些版本未经该协商就发送压缩证明，Kryptex 回复"已接受"但不计入份额。请更新到 1.4.4 或更高版本。

### 7. 超频（锁定核心、显存、V/F 偏移）

内核在 L2 缓存中运行，几乎不需要显存带宽，因此显存频率可以降到最低。
关键的三项设置：**锁定的核心频率**、**较低的显存频率**和 **V/F 偏移**（降压：同频率下更低的电压；`--auto-uv` 会自动找到，见第 12 节）。

**FreeForGemPear 实测，RTX 3070：**核心锁定 `1560` MHz，显存 `810` MHz，功耗限制 `120` W →
**约 70 TH/s，约 105–107 W**；随后 `--auto-uv` 找到 +195..240（约 86–107 W）。

**各显卡起始值。***参考*行是这些显卡已公开的调校结果；*估计*行根据同代显卡推算，未经实测。每块卡都不同：让 `--auto-uv` 找出你自己的偏移。

| 显卡 | 核心锁定，MHz | 显存 | V/F 偏移，MHz | 来源 |
|---|---|---|---|---|
| RTX 3060 Ti、RTX 3070、RTX 3070 Laptop、RTX 3080 Ti | 1560–1650 | 锁定 `810` | +200..255 | 参考（3070 为实测） |
| RTX 3060、RTX 3070 Ti、RTX 3080、RTX 3090 | 1560–1650 | 锁定 `810` | +200..255 | 估计 |
| CMP 70HX、CMP 90HX（Ampere） | 1650 | 偏移 `-2000`（`--moffset`） | +255..300 | 参考，未用 FreeForGemPear 验证 |
| RTX 4070 Ti、RTX 4090 | 2445 | 锁定 `810` | +315..345 | 参考 |
| RTX 4060、4060 Ti、4070、4070 Super、4070 Ti Super、4080、4080 Super | 1900–2445 | 锁定 `810` | +300..345 | 估计 |
| RTX 5070 Ti | 2445 | 锁定 `810` | +480 | 参考 |
| RTX 5070 | 2400 | 锁定 `810` | 检查通过至 +390，更高则崩溃 | 用户报告，FreeForGemPear 1.5.1，Windows，驱动 617.14 |
| RTX 5060、5060 Ti、5080、5090 | 1900–2500 | 锁定 `810` | +390..480 | 估计 |

RTX 40 和 50 的锁定核心频率高得多（**1900–2500 MHz**），能承受的偏移也比 RTX 30 更高；因此 auto-uv 在它们上面从 +200 开始搜索（RTX 30 从 +120）。
Turing / CMP 30HX–50HX（sm_75）和 CMP 170HX / A100（sm_80）不受支持。

* HiveOS：Overclock 选项卡 → 核心频率（绝对值）、显存频率（绝对值）、功耗限制；V/F 偏移用 `--auto-uv`。
* Linux：`nvidia-smi -lgc 1560,1560`、`nvidia-smi -lmc 810,810`、`nvidia-smi -pl 120`（root），或 `--lock-cclock` / `--lock-mclock`。
* Windows：在 `start.bat` 中设置 `LOCK_CCLOCK` / `LOCK_MCLOCK` / `MOFFSET`（以管理员身份），需要功耗限制时用 `start-nvidia-smi.bat`，
  或 MSI Afterburner。各显卡的现成组合见第 12 节 *Windows 现成组合*。

如果份额被拒绝或日志出现 `bad` 命中，请降低核心频率或偏移。

### 8. 如何阅读日志

| 行 | 含义 |
|---|---|
| `FreeForGemPear 1.4.9 \| pool ... \| worker ... \| N GPU(s)` | 启动横幅 |
| `[pearlgpu] search kernel: SASS-patched cubin (...)` | 已加载调优内核。`embedded search kernel could not be loaded (原因)` = 矿工报错退出（驱动低于 525 或显卡不受支持） |
| `new job ... prepared in ...` | 新的矿池任务已就绪 |
| `[ 120s] GPU0 ... \| total X TH/s \| hits ... \| user sub S acc A rej R \| fee sub ... acc ... rej ...` | 每 10 秒：每块显卡和总算力；你和抽水的已提交 / 已接受 / 已拒绝份额；抽水时段显示 `[DEVFEE]` |
| `[DISCONNECTED]` | 与矿池断开；矿工会自动重连 |
| `devfee: ...` | 抽水时段（第 10 节） |
| `!! ...` | 警告或错误（GPU 工作线程停止、某个 hit 未提交） |

### 9. 校验下载（SHA-256）

将结果与发布页面上的哈希比对。

* Linux：`sha256sum freeforgempear-1.5.1-linux-x86_64.tar.gz freeforgempear-1.5.1.tar.gz`
* Windows：`certutil -hashfile freeforgempear-1.5.1-windows-x64.zip SHA256` 或
  `Get-FileHash freeforgempear-1.5.1-windows-x64.zip -Algorithm SHA256`

### 10. 开发者抽水 — 1%

每 100 分钟一个 60 秒时段，位于周期内的随机时刻，在**你配置的同一个矿池**上，使用单独的登录（矿工名 `ffgp-devfee`）。
日志显示 `devfee: next slot in N min`、`devfee: mining 60s for FreeForGemPear dev (1%)` 和 `devfee: back to user`。
第一个时段出现在最初 100 分钟内的随机时刻。如果抽水登录无法服务，该时段会被跳过；你的挖矿不会被阻塞。
自 1.4.2 起，在 LuckyPool 上抽水登录同时为开发者合并挖 NOCK（`prl1...+NOCK`）；若矿池拒绝该登录，矿工会退回普通的 PRL 抽水钱包。抽水时间仍为 1%。
抽水是许可证的条件：不得移除、降低或重定向（见 [LICENSE](LICENSE) 第 3 条）。

### 11. 故障排除

| 症状 | 解决办法 |
|---|---|
| 检查一切是否正常 | `freeforgempear --self-test`（Windows：`freeforgempear.exe --self-test`）应以 `== self-test PASSED ==` 结束 |
| 启动时出现 `embedded search kernel could not be loaded` | 更新 NVIDIA 驱动（525+；**RTX 50xx 需要 570+**），并确认显卡为 Ampere（RTX 30xx）或更新 |
| CPU 占用高 | 默认 GPU 等待为休眠（每块显卡远低于一个核心的 10%）。`PEARL_SPIN=1` 会改为忙等（每块显卡占满一个核心）：确认没有设置 |
| `pg_init(...)` 或显存不足错误 | 其他程序占用了显卡（游戏、本地 AI 模型）。关闭它，或用 `--devices` 选择显卡 |
| 长时间 `[DISCONNECTED]` | 检查矿池主机/端口和防火墙（出站 TCP）；试试第 6 节的其他区域 |
| 大量被拒绝的份额 | 降低核心频率（第 7 节），检查散热 |
| HiveOS 提示 `Already installed` 且没有变化 | Installation URL 必须指向新版本 |
| Windows：窗口一闪而过 | 在 `cmd` 中运行 `start.bat` 查看信息；路径不得含空格和非英文字符 |
| Windows：`.exe` 被 Defender / SmartScreen 拦截 | 点击"更多信息 → 仍要运行"，或将文件夹加入排除项；核对 SHA-256 |

环境变量（启动矿工前设置）：`PEARL_SPIN=1`（忙等代替休眠）。
`FFGP_M=N` 设置每次尝试的 A 行数（须为 128 的倍数）。`m` 默认值为 47104，矿工会根据每块 GPU 的 SM 数自动选取 `m`（`m = 128 * SM * r`），因此混合 SM 的矿机能让每张显卡各取所需——按 SM 分组各建一个任务（1.4.7 起）。仅在实验时才设置 `FFGP_M`：它对所有设备统一 `m`；`m` 越大算力略高，但会占用更多主机内存。
Windows：在 `cmd` 窗口中 `set PEARL_SPIN=1`，然后在同一窗口中运行 `start.bat`。

### 12. 降压 / `--auto-uv`（可选，1.4.0 新增）

**默认关闭。** 启用 `--auto-uv` 后，矿工会为每块显卡找出一个更低的核心电压设置（显卡的 *V/F 偏移*），在该设置下显卡
仍能全部计算正确；挖矿时使用它，退出时恢复你自己的设置。电压更低意味着在相同算力下功耗更少：频率仍保持你锁定的值（第 7 节）。

**它做什么**
1. 读取显卡当前的 V/F 偏移（HiveOS 超频、你自己的脚本等），并在做任何改动*之前*把它写入恢复文件。
2. 先让显卡预热 3 分钟，然后以 **15 MHz** 为步长提高偏移。每一步在单独的进程中检查约 30 秒：同一个任务计算数千次，
   每个结果的每个 tile 逐位比较；比较正常挖矿尝试的命中计数；找到的结果由官方验证器在 CPU 上验证。接近极限时会出现
   驱动不报任何错误的错误结果，因此检查同时在 GPU 和 CPU 上进行。
3. 第一次不一致、CUDA 错误、Xid 错误或卡死就结束爬升。结果为**最后一个正常步长再减一步（15 MHz 余量）**，并用更长的
   检查（约 2 分钟）确认；确认失败则再降一步。最高尝试到 +495（`--uv-max`，最多 +600；1.5.1 之前为 +300，硬上限 +400）。RTX 40/50 显卡常能稳定在 +450..480。
4. 结果保存在 `uv.json` 中，以 GPU UUID、驱动版本和内核版本为键。之后启动时，经过快速检查（约 30 秒）后直接应用。
   如果驱动、内核版本或你的起始偏移发生变化，则重新调校。
5. 挖矿期间：每 6 小时（`--uv-reval-hours`）重新检查一次（该显卡的挖矿暂停约 30 秒）。如果找到的结果未通过 CPU 验证，
   或出现一批被拒绝的份额，偏移下调一步（不会低于你自己的值）并保存。如果别人修改了偏移（例如 HiveOS 重新应用超频），
   auto-uv 会在该显卡上退让，不会与之对抗。
6. 退出时（Ctrl+C、SIGTERM、正常结束）恢复你的偏移。如果矿工被强制终止（`kill -9`、崩溃），下次启动会先根据恢复文件
   `uv-restore.json` 恢复你的偏移。重启本身也会重置驱动中的偏移。

只修改 V/F 偏移：锁定频率、功耗上限和显存都不会被改动。只会改动矿工正在挖矿的显卡。NVML 设备编号与 CUDA 编号不同，
因此每块显卡都通过 PCI 总线地址（和 UUID）匹配；日志中每次改动都会显示总线地址。

**要求。** NVIDIA 驱动（NVML 在运行时加载，无需安装任何东西），Linux / HiveOS 上需要 **root**（那里矿工以 root 运行），
Windows 上需要**管理员权限**。没有 NVML 或权限时，矿工会在日志中说明原因并照常挖矿。**耗时：**首次调校每块显卡约
**15 分钟**（视步数而定，10-25 分钟；矿机上的各显卡并行调校，完成后才开始挖矿）。之后每次启动增加约 30 秒。

**Windows 分步说明。** 已在 Windows 11、GeForce RTX 3070、驱动 581.80 上验证：驱动允许矿工读取并修改偏移（日志显示
`allowed -1000..1000`），调校找到 +240（原为 +225，同频下 98.4 W → 86.5 W），保护机制也生效了（一个找到的结果未通过 CPU
校验 → 退回 +225，该卡 auto-uv 暂停 24 小时）。
1. **管理员。** 右键 `.bat` →*以管理员身份运行*。开机自启时，在计划任务中勾选*使用最高权限运行*。没有权限时日志会说明
   无法修改偏移，矿工照常挖矿。
2. **锁定核心频率**（第 7 节）：在 `start.bat` 中 `set "LOCK_CCLOCK=1560"`（一个值，或按显卡用逗号分隔），或在
   `start-nvidia-smi.bat` 中用 `nvidia-smi -lgc`。锁频后，更高的偏移意味着同频下更低的电压；不锁频时显卡可能改为提升频率，节省更少。
3. **开启**，任选其一：
   * `start.bat`（或任一 `start-luckypool-*` / `start-kryptex-*`）：删除 `set "AUTO_UV=1"` 前面的 `rem`。上限为
     `set "UV_MAX="`（留空 = +495；RTX 40/50 常能稳定在 +450..480，RTX 30 通常 +200..360）。
   * `start-nvidia-smi.bat`：`AUTO_UV=1`，范围在 `UV_MIN` / `UV_MAX`，并通过 `nvidia-smi` 设置功耗限制和锁频。
   * 自己的命令行：`freeforgempear.exe --pool ... --wallet ... --auto-uv --uv-max 480`。
4. **起点。** 调校从显卡当前的偏移开始，且不会低于它。MSI Afterburner（或类似工具）中的 *Core Clock* 偏移就是同一个偏移：
   你在那里设置的值就是起点，退出时会恢复。挖矿期间不要让 Afterburner 重新应用配置文件：auto-uv 发现偏移被别人修改后会在
   该卡上退让。未设置偏移时，调校从 +120 开始，RTX 40/50 从 +200 开始（或 `--uv-min` / `UV_MIN`）。
5. **等待。** 首次启动时每块显卡调校约 15 分钟（所有显卡并行），完成后才开始挖矿。之后的启动在约 30 秒检查后应用已保存的值。
6. **文件**在矿工目录中：`uv.json`（结果）和 `uv-restore.json`（你原来的偏移）。该目录必须可写，所以不要把矿工放在
   `C:\Program Files`。重新调校：关闭矿工，删除 `uv.json`（或在该目录运行 `freeforgempear.exe --uv-reset`），再启动。
   新驱动或新的矿工内核会自动触发重新调校。
7. **查看结果：**`findstr auto-uv logs\*.log`（`start-nvidia-smi.bat` 为 `logs\ffgp.log`）。那块 RTX 3070 的示例：
```
auto-uv: GPU0 (bus 04, RTX 3070): CUDA device 0 -> NVML index 0, PCI 0000:04:00.0, ..., current offset +225 (allowed -1000..1000)
auto-uv: GPU0 (bus 04, RTX 3070) step +255: ok (6560 reruns, cpu ok 6), 94.0 W, 1560 MHz, 58 C
auto-uv: GPU0 (bus 04, RTX 3070) step +270: FAILED (rerun_diff: 256 transcript word(s) differ between reruns of seed 3 ...)
auto-uv: GPU0 (bus 04, RTX 3070) offset +225 -> +240, 98.4 W -> 86.5 W, saved
```
   * `auto-uv paused for this GPU for 24 h`：显卡在找到的偏移上出错；它以你自己的偏移挖矿，24 小时后（或 `--uv-reset` 后）再次调校。
   * `start offset ... leaves no room below the cap`：你的偏移已等于或高于 `UV_MAX`；请提高 `UV_MAX`。
   * `no safe offset above ... was found`：显卡承受不了比你的值更高的偏移；不做任何修改。
8. **如果电脑卡死或驱动重置**（黑屏、“显示驱动程序已停止响应”）：重启后再次启动矿工——它会先从 `uv-restore.json`
   恢复你的偏移。如果矿工本身在调校某一步时崩溃（窗口显示 `Miner exited (code -1073741819)`），下次启动会把这一步视为失败，只在其下方调校（自 1.5.2 起；之前会重新开始整个搜索并在同一步再次崩溃）。如果某块卡反复出错，请降低 `UV_MAX` 或不要开启 `AUTO_UV`。

**Windows 现成组合。** 右键 `start.bat` →*编辑*，删除 `set "AUTO_UV=1"` 前面的 `rem`，填写下面的字段，保存，并**以管理员身份**运行。
留空的字段使用默认值。数值为起始值，取自第 7 节的表格。
```bat
rem --- RTX 30 (3060 Ti / 3070 / 3080 ...) ---
set "AUTO_UV=1"
set "LOCK_CCLOCK=1560"
set "LOCK_MCLOCK=810"
set "UV_MIN=150"
set "UV_MAX=360"

rem --- RTX 40 (4060 ... 4090) ---
set "AUTO_UV=1"
set "LOCK_CCLOCK=2445"
set "LOCK_MCLOCK=810"
set "UV_MIN=200"
set "UV_MAX=450"

rem --- RTX 50 (5060 ... 5090): start higher, the search is shorter ---
set "AUTO_UV=1"
set "LOCK_CCLOCK=2445"
set "LOCK_MCLOCK=810"
set "UV_MIN=300"
set "UV_MAX=495"
```
* **混合矿机：**`LOCK_CCLOCK` / `LOCK_MCLOCK` 按 `DEVICES` 顺序每卡一个值，例如 `set "LOCK_CCLOCK=1560,2445"`。
  `UV_MIN` / `UV_MAX` 对所有显卡通用：留空（RTX 40/50 会自动从 +200 开始）或使用较弱显卡的值。
* **更快调校：**`set "UV_STEP=30"` 使步数减半（结果更粗）。在 RTX 50 上，较高的 `UV_MIN`（约 +300）最省时间：显卡反正能承受，搜索直接从那里开始。不要设得过高：如果显卡在 `UV_MIN` 处就未通过检查，则不会调校，保持你自己的偏移（日志会说明；此时请降低 `UV_MIN`）。
* **已知道偏移：**`set "UV_OFFSET=480"`（不开启 `AUTO_UV`）——一次快速检查后直接应用，不搜索。
* **还需要功耗限制：**使用 `start-nvidia-smi.bat`（`CCLOCK` / `MCLOCK` / `PLIMIT`，`UV_MIN` / `UV_MAX` / `UV_STEP`）。
* **更改范围后重新调校：**在一次启动中设置 `set "EXTRA=--uv-reset"`，之后再清空。

**风险。** 降压超过显卡的承受能力可能导致驱动重置或卡死（调校只会走到刚刚检查通过的值并留有余量，但芯片、温度和驱动各不相同）。
卡死后请重启：下次启动会首先恢复你的偏移。风险自负，与任何超频一样。

**启用**
* HiveOS：在飞行表的 *Extra config arguments* 中加入 `--auto-uv`（详见 HIVEOS.md）。
* Linux：`sudo ./freeforgempear --pool ... --wallet ... --auto-uv`
* Windows：右键 `start.bat` → *编辑*，删除 `set "AUTO_UV=1"` 前面的 `rem`；**以管理员身份**运行 `start.bat`。

| 选项 | 含义 |
|---|---|
| `--auto-uv` | 查找、应用并监控降压（如上所述） |
| `--uv-offset N` | 快速检查后应用固定偏移 N MHz，不做调校（检查失败则保持你的偏移） |
| `--uv-reset` | 忘记已保存的结果；与 `--auto-uv` 一起使用时重新调校；没有钱包地址时矿工直接退出 |
| `--uv-max N` | 调校尝试的最高偏移（默认 495，最多 600；Windows：在 `start.bat` 中 `set "UV_MAX=480"`） |
| `--uv-min N` | 调校的起始偏移，且不会低于该值（默认：HiveOS/驱动中的偏移，否则 120） |
| `--uv-step N` | 每步调校的 MHz（默认 15） |
| `--uv-keep-last` | 某一步失败后保留最后一个通过的步（默认：最后通过的步再减一步） |
| `--uv-reval-hours H` | 挖矿期间的复查间隔（默认 6，`0` 表示不复查） |
| `--uv-dir DIR` | `uv.json` 和 `uv-restore.json` 的存放位置（也可用 `FFGP_STATE_DIR`） |
| `--auto-uv-dry-run` | 只在日志中说明会改动哪些显卡（CUDA 设备 → NVML 设备、总线地址），不做任何改动 |

**GPU 频率控制（可选，基于 NVML；需要 root / 管理员权限，Windows 上可用 — 1.4.9 新增）。** `--lock-cclock N` 将核心频率锁定为 N MHz（类似 `nvidia-smi -lgc N,N`），`--lock-mclock N` 将显存频率锁定为 N MHz（类似 `-lmc`），`--moffset N` 设置显存频率偏移（MHz），可以为负数（需要驱动 555+）。每个选项可对所有 GPU 使用同一个值，也可按 `--devices` 顺序给出逗号分隔的列表（每个设备一个值）。设置在启动时应用，退出时（Ctrl-C / SIGTERM / 崩溃）自动撤销。Windows 上请在 `.bat` 文件中设置 `LOCK_CCLOCK`、`LOCK_MCLOCK`、`MOFFSET`。示例：`--lock-cclock 1500 --lock-mclock 810 --moffset -200`。

文件默认位置：程序旁边（Windows：矿工文件夹）；HiveOS 上为 `/hive-config/freeforgempear`（矿工更新和重启后仍保留）。
**重置：** `freeforgempear --uv-reset`（或删除 `uv.json`）。

**日志行**（都以 `auto-uv:` 开头，`grep auto-uv` 即可看到全过程）：
```
auto-uv: GPU0 (bus 0d, RTX 3070): CUDA device 0 -> NVML index 1, PCI 0000:0D:00.0, GPU-0d06..., current offset +150
auto-uv: GPU0 (bus 0d, RTX 3070) step +195: ok (1952 reruns, cpu ok 6), 110.4 W, 1560 MHz, 53 C
auto-uv: GPU0 (bus 0d, RTX 3070) step +225: FAILED (rerun_diff: ...)
auto-uv: GPU0 (bus 0d, RTX 3070) offset +150 -> +195, 116.6 W -> 106.6 W, saved
auto-uv: GPU0 (bus 0d, RTX 3070) offset +195 -> +150 restored
```

**参考值，RTX 3070 锁定 1560 MHz、显存 810 MHz**（任何偏移下都约 70 TH/s，算力不变）：+150 时约 116 W，+180 时约 113 W，
+195 时约 110 W，+210 时约 107–108 W。在我们的测试卡上，内置检查从 +225 起发现了静默计算错误（较短的手动测试甚至在 +255 也通过），
因此调校结果为 +195…+210。静默错误不会产生 Xid 或拒绝份额，但会丢失有效份额：看似“稳定”的更高偏移反而可能收益更低。
每颗芯片的极限不同，也随温度变化：相比默认偏移，预计每块显卡节省约 5–10 W；请用 `--uv-offset N` 验证自己的数值，不要照搬别人的数值。

**手动调校（不使用 `--auto-uv`）**，适合喜欢用 HiveOS 超频页或 Afterburner 的用户：
1. 按第 7 节锁定核心频率并降低显存频率，让显卡预热几分钟。
2. 将核心频率**偏移**（V/F 偏移：HiveOS Overclock 标签、Afterburner 或 `--uv-offset N`）提高 15 MHz。
3. 至少运行 30 分钟并观察：`bad` 必须保持为 0，`rej` 不应增长，日志中不应出现 `DEAD`，`dmesg` / HiveOS 消息中不应出现 `Xid`
   （13、31、43、79 ...）。`freeforgempear --uv-offset N` 会应用 N，运行与 `--auto-uv` 相同的检查并用它挖矿，是测试候选值的快捷方式。
4. 从第 2 步重复，直到第一次出现问题，然后从出现问题的偏移回退**两步（30 MHz）**并保留该值。
5. 驱动更新、更换散热器或天气炎热后重新测试。
