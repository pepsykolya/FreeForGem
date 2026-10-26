<!-- FreeForGem — see LICENSE -->
# FreeForGem — Manual

* [English](#english)
* [Русский](#русский)
* [中文](#中文)

---

## English

### 1. What it is

FreeForGem is a GPU miner for **Pearl (PRL)** (pearlhash proof-of-useful-work) on NVIDIA GPUs (and, since 1.6.0, AMD Radeon
on Windows), with a **1% developer fee**. Since 1.6.0 it also mines Common Foundry (CMFD, section 13), since 1.7.0 **NOID**
(Parano1d, section 15).

**Name.** Since 1.7.0 the multi-coin miner (Pearl + CMFD + NOID) is called **FreeForGem** (binary `freeforgem`, HiveOS custom
miner `freeforgem`, repository `pepsykolya/FreeForGem`). **FreeForGemPear** continues as the Pearl miner under its own name.
Moving from FreeForGemPear: on HiveOS create a **new flight sheet** (Miner name `freeforgem` + the new Installation URL); the
saved undervolt results (`uv.json` of `/hive-config/freeforgempear` or of a `freeforgempear` folder next to the new one) are
imported automatically on the first start (the log says `imported the saved undervolt results of FreeForGemPear`).
It has its own CUDA kernel for Ampere: the A′ matrix is generated on-chip, the int8 tensor-core GEMM runs in a persistent
kernel, and the jackpot fold and the BLAKE3 hash are fused into it. The main loop is hand-tuned at SASS level.
The host builds and checks every share proof with the official `zk-pow` verifier before it is submitted.

| GPU series | Status |
|---|---|
| RTX 30xx (Ampere, sm_86): 3060 / 3060 Ti / 3070 / 3070 Ti / 3080 / 3090 | **main target, tuned: ~70 TH/s on an RTX 3070 at ~107 W** |
| RTX 40xx (Ada, sm_89) | own SASS kernel since 1.5.4 (RTX 4070 Ti: same hashrate, ~10 W less than the RTX 30xx kernel) |
| RTX 50xx (Blackwell, sm_120) | supported since 1.5.0 with a compiled kernel (untuned); needs driver 570+ |
| CMP 50HX (TU102) | runs, but **not recommended for Pearl: ~2.6 TH/s per card** (int8 tensor cores cut ~17× against the CMP 40HX, on any driver) |
| RTX 20xx / GTX 16xx / CMP 30HX–50HX (Turing, sm_75) | supported since 1.5.3 with an own SASS kernel: ~49.9 TH/s on an RTX 2060 SUPER @1605 MHz; **memory must be locked at 5001, not 810** |
| AMD Radeon RX 6000 (RDNA2), Windows | **new in 1.6.0**, own OpenCL kernel: 34.4 TH/s on an RX 6800 XT (section 14) |
| Older GPUs, A100 (sm_80), AMD on Linux / HiveOS | not supported |

### 2. Requirements

* NVIDIA GPU: RTX 30 / RTX 40 (own kernels), RTX 20 / GTX 16 / CMP, RTX 50 (since 1.5.0); NVIDIA driver **525 or newer** (RTX 50xx: **570+**).
* Or, on **Windows only**, an **AMD Radeon RX 6000** (RDNA2) card with the AMD Adrenalin driver (since 1.6.0, section 14).
* HiveOS, Linux x86_64 (glibc 2.31+, Ubuntu 20.04+), or Windows 10/11 x64. No CUDA toolkit, no Python, no WSL.
* A PRL wallet address (`prl1...`) and an internet connection to a Pearl stratum pool.
* Video memory: the kernel works from the L2 cache and needs little VRAM.

### 3. HiveOS setup (step by step)

FreeForGem runs on HiveOS as a **Custom miner**: no SSH and no manual installation.

**Installation URL (1.4.9):**

```
https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.2-beta/freeforgem-1.7.2beta.tar.gz
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
   | Miner name | `freeforgem` (filled in automatically after you paste the URL) |
   | Installation URL | the link above |
   | Hash algorithm | `pearlhash` (or leave `----`) |
   | Wallet and worker template | `%WAL%.%WORKER_NAME%` |
   | Pool URL | `pearl-eu2.luckypool.io:3360`, or Kryptex `prl-eu.kryptex.network:7048` (any pool from section 6) |
   | Pass | leave empty |
   | Extra config arguments | optional, for example `--devices 0,1` |

4. **Apply.** *Apply changes*, *Create flight sheet*, then apply it to the rig.

The miner log is at `/var/log/miner/freeforgem/freeforgem.log` and in the HiveOS miner console.
The dashboard shows the hashrate per GPU and in total, temperatures, fans, and accepted and rejected shares.
To update, change the Installation URL to the new version. The same guide with more details: [HIVEOS.md](HIVEOS.md).

### 4. Linux without HiveOS

```
tar xzf freeforgem-1.7.2-beta-linux-x86_64.tar.gz
cd freeforgem-1.7.2-beta
./freeforgem --self-test        # optional: checks the GPU kernel against the CPU reference
./freeforgem --pool pearl-eu2.luckypool.io:3360 --wallet YOUR_PRL_ADDRESS --worker rig1     # LuckyPool
./freeforgem --pool prl-eu.kryptex.network:7048 --wallet YOUR_PRL_ADDRESS --worker rig1       # Kryptex
```

| Option | Meaning |
|---|---|
| `--pool HOST:PORT` | stratum pool (default `pearl-eu2.luckypool.io:3360`); a `stratum+tcp://` prefix is accepted, SSL is not supported (use the plain TCP port) |
| `--wallet ADDRESS` | your PRL address (required) |
| `--worker NAME` | worker name (default `worker1`) |
| `--devices 0,1,...` | GPUs to use (default: all visible; honours `CUDA_VISIBLE_DEVICES`) |
| `--minutes N` | stop after N minutes (default: run until Ctrl-C / SIGTERM) |
| `--self-test` | verify the kernel and exit |

One process drives all GPUs and reconnects to the pool on its own. For a permanent setup run it under `systemd`, `screen` or `tmux`.

### 5. Windows 10/11

Native Windows build: no WSL, no CUDA installation, no Python. Only the NVIDIA driver is needed.

1. Download **`freeforgem-1.7.2-beta-windows-x64.zip`** from the release page:
   `https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.2-beta/freeforgem-1.7.2-beta-windows-x64.zip`
   and check its SHA-256 (section 9).
2. Unpack it to a folder **without spaces or non-English letters**, e.g. `C:\FreeForGem`.
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
* Without `start.bat`: `freeforgem.exe --pool pearl-eu2.luckypool.io:3360 --wallet YOUR_PRL_ADDRESS --worker rig1`
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
cd /d C:\Mining\freeforgem-1.7.2-beta-windows-x64
:loop
powershell -NoProfile -Command "& { .\freeforgem.exe --pool pearl-eu2.luckypool.io:3360 --wallet YOUR_PRL_ADDRESS --worker rig1 --auto-uv --uv-min 135 --uv-max 360 | Tee-Object -Append -FilePath C:\Mining\ffgp.log }"
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

**Measured with FreeForGem, RTX 3070:** core `1560` MHz locked, memory `810` MHz, power limit `120` W →
**~70 TH/s at ~105–107 W**; `--auto-uv` then finds +195..240 (≈ 86–107 W).

**Starting values per card.** *Reference* rows are published tuning results for that card; *estimate* rows are derived from
cards of the same generation and are not measured. Every card differs: let `--auto-uv` find your own offset.

| GPU | Core lock, MHz | Memory | V/F offset, MHz | Source |
|---|---|---|---|---|
| RTX 3060 Ti, RTX 3070, RTX 3070 Laptop, RTX 3080 Ti | 1560–1650 | lock `810` | +200..255 | reference (3070: measured) |
| RTX 3060, RTX 3070 Ti, RTX 3080, RTX 3090 | 1560–1650 | lock `810` | +200..255 | estimate |
| CMP 70HX, CMP 90HX (Ampere) | 1650 | offset `-2000` (`--moffset`) | +255..300 | reference, not checked with FreeForGem |
| RTX 4070 Ti, RTX 4090 | 2445 | lock `810` | +315..345 | reference |
| RTX 4060, 4060 Ti, 4070, 4070 Super, 4070 Ti Super, 4080, 4080 Super | 1900–2445 | lock `810` | +300..345 | estimate |
| RTX 5070 Ti | 2445 | lock `810` | +480 | reference |
| RTX 5070 | 2400 | lock `810` | checks pass up to +390, crash above | user report, FreeForGemPear 1.5.1, Windows, driver 617.14 |
| RTX 5060, 5060 Ti, 5080, 5090 | 1900–2500 | lock `810` | +390..480 | estimate |
| RTX 2060 SUPER, RTX 2070/2080 (incl. SUPER), GTX 1660/1650 | 1500–1650 | lock `5001` (**not 810**: Turing loses ~25% at 810) | +200..260; auto-uv limit +225 by default (since 1.6.2) | reference (2060 SUPER: measured) |
| CMP 40HX | 1400 | lock `5001` | +150; auto-uv limit +150 by default (since 1.6.2) | user rigs; a CMP 40HX hit Xid at +255 |
| CMP 50HX (TU102) | — | lock `5001` | +220 and more; auto-uv limit +225 by default (`--uv-max 225`) | user rigs. **Not recommended for Pearl: ~2.6 TH/s per card** (int8 tensor cores cut ~17× against the 40HX; the same on drivers 550.78 and 595.91.07, 3× CMP 50HX rig) |

RTX 40 and 50 run at a much higher locked core clock (**1900–2500 MHz**) and hold a higher offset than RTX 30; that is why
auto-uv starts their search at +200 on RTX 40 and +300 on RTX 50 (RTX 30: +120).

**Mixed rig:** `--auto-uv` is enough — the limits are chosen per card automatically (since 1.6.2): RTX 20xx / GTX 16xx up to
+225, CMP 30HX/40HX up to +150 (CMP 50HX: +225), all others up to +495; the log shows the range of every GPU at start
(`auto-uv: GPU5 (bus 0b, CMP 40HX): range +120..+150 (max: CMP default; start: default)`). For manual control give one value
per GPU in `--devices` order: `--uv-max 225,495,150` (`-` = that GPU's default). Turing (RTX 20 / GTX 16 / CMP 30HX–50HX, sm_75) is supported since 1.5.3: unlike RTX 30 it needs the memory locked at 5001 (see the table); CMP 170HX / A100 (sm_80) are not supported.

**CMP cards (CMP 30HX / 40HX / 50HX).** CMP 30HX/40HX mine Pearl with the Turing kernel (memory locked at 5001, auto-uv
limit +150). **CMP 50HX: not recommended for Pearl** — only ~2.6 TH/s per card (its int8 tensor cores are cut ~17x against the
CMP 40HX, on every driver). **For Common Foundry (CMFD) the CMP 50HX works well:** ~13 FW/s per card at ~220 W (power limit
225 W, core lock 1410 MHz, memory not locked; 4x CMP 50HX rig on the Aria pool), with `--auto-uv` (CMFD tuner, +225) about
13.2 FW/s at 196–203 W (+8–11 % FW/kWh); see section 13. CMFD needs ~7 GB of free GPU memory, so 6 GB cards (CMP 30HX) cannot
run it. **Unlocking CMP cards is not part of FreeForGem:** we do not provide or support drivers or tools that lift the
limits of CMP cards; all numbers here are for the cards as they are.

* HiveOS: Overclock tab → core clock (absolute), memory clock (absolute), power limit; the V/F offset via `--auto-uv`.
* Linux: `nvidia-smi -lgc 1560,1560`, `nvidia-smi -lmc 810,810`, `nvidia-smi -pl 120` (root), or `--lock-cclock` / `--lock-mclock`.
* Windows: `LOCK_CCLOCK` / `LOCK_MCLOCK` / `MOFFSET` in `start.bat` (as Administrator), `start-nvidia-smi.bat` for a power
  limit, or MSI Afterburner. Ready combinations per card: section 12, *Windows: ready combinations*.

If shares get rejected or the log shows `bad` hits, lower the core clock or the offset.

### 8. Reading the log

| Line | Meaning |
|---|---|
| `FreeForGem 1.7.2-beta \| pool ... \| worker ... \| N GPU(s)` | start banner |
| `[pearlgpu] search kernel: SASS-patched cubin (...)` | the tuned kernel is loaded. `embedded search kernel could not be loaded (reason)` = the miner stops with an error (driver older than 525 or an unsupported GPU) |
| `new job ... prepared in ...` | a new pool job is ready |
| `[ 120s] GPU0 ... \| total X TH/s \| hits ... \| user sub S acc A rej R \| fee sub ... acc ... rej ...` | every 10 s: hashrate per GPU and in total; shares submitted / accepted / rejected for you and for the dev fee; `[DEVFEE]` while a fee slot runs Since 1.5.3 each GPU also shows its board power and efficiency, e.g. `GPU0 49.906 (151 W, 0.330 TH/W)`, and the total `\| 151 W, 0.330 TH/W` (when NVML is available) |
| `[DISCONNECTED]` | the pool connection is down; the miner reconnects by itself |
| `devfee: ...` | the dev fee slots (section 10) |
| `!! ...` | a warning or error (a GPU worker stopped, a hit was not submitted) |

### 9. Verifying the download (SHA-256)

Compare the result with the hash on the release page.

* Linux: `sha256sum freeforgem-1.7.2-beta-linux-x86_64.tar.gz freeforgem-1.7.2beta.tar.gz`
* Windows: `certutil -hashfile freeforgem-1.7.2-beta-windows-x64.zip SHA256` or
  `Get-FileHash freeforgem-1.7.2-beta-windows-x64.zip -Algorithm SHA256`

### 10. Dev fee — 1%

One 60-second slot in every 100 minutes, at a random moment of the cycle, on **the same pool you configured**, under a
separate login (worker `ffgp-devfee`). The log shows `devfee: next slot in N min`,
`devfee: mining 60s for FreeForGem dev (1%)` and `devfee: back to user`. The first slot comes at a random moment within
the first 100 minutes. If the fee login cannot be served, the slot is skipped; your mining is never blocked.
On LuckyPool the fee login also merge-mines NOCK for the developer (`prl1...+NOCK`, since 1.4.2); if the pool refuses that login,
the miner falls back to the plain PRL fee wallet. The fee time stays 1%.
The fee is a condition of the license: removing, reducing or redirecting it is not permitted (see [LICENSE](LICENSE), clause 3).

### 11. Troubleshooting

| Symptom | Fix |
|---|---|
| Check that everything works | `freeforgem --self-test` (Windows: `freeforgem.exe --self-test`) must end with `== self-test PASSED ==` |
| `embedded search kernel could not be loaded` at start | update the NVIDIA driver (525+; **RTX 50xx needs 570+**) and check that the GPU is Ampere (RTX 30xx) or newer |
| High CPU load | by default GPU waits sleep (well under 10% of a core per GPU). `PEARL_SPIN=1` makes them spin (a full core per GPU): make sure it is not set |
| `pg_init(...)` or out-of-memory errors | another program holds the GPU (games, local AI models). Close it or choose GPUs with `--devices` |
| `[DISCONNECTED]` for a long time | check the pool host/port and the firewall (outgoing TCP); try another region from section 6 |
| Many rejected shares | lower the core clock (section 7), check cooling |
| Hashrate drops after a few minutes, Task Manager shows *Video Encode* load | screen sharing, recording or streaming (Discord screen share, OBS, NVIDIA App / Instant Replay) runs on the same GPU and takes its share; stop it or turn off hardware acceleration in that program |
| HiveOS says `Already installed` and nothing changes | the Installation URL must point to a new version |
| Windows: the window closes at once | run `start.bat` from `cmd` to see the message; the path must not contain spaces or non-English letters |
| Windows: `.exe` blocked by Defender / SmartScreen | *More info → Run anyway*, or add the folder to the exclusions; check the SHA-256 |

Environment variables (set before starting the miner): `PEARL_SPIN=1` (spin-wait instead of sleeping).
`FFGP_M=N` sets the rows of A per attempt (a multiple of 128). By default `m` is 47104 and the miner picks `m` for each GPU from its SM count (`m = 128 * SM * r`), so a mixed-SM rig balances every card — one job per SM group, each card mines its own (new in 1.4.7). Set `FFGP_M` only to experiment: it forces one `m` on every device, and a larger `m` gives slightly higher throughput at the cost of more host RAM.
On Windows: `set PEARL_SPIN=1` in a `cmd` window, then run `start.bat` from that window.

### 12. Undervolt / `--auto-uv` (optional, new in 1.4.0)

**Video tutorials.** Step-by-step videos on setting up the undervolt are attached to the GitHub release as separate files (Releases page → the release → *Assets*): in English `FreeForGemPear-1.5.3-undervolt-EN.mp4` and in Russian `FreeForGemPear-1.5.2-undervolt-RU.mp4`.

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
   value tried is +495 (`--uv-max`, at most +600; before 1.5.1 it was +300 with a hard cap of +400); since 1.6.2 it is +225 on
   Turing (RTX 20xx / GTX 16xx) and +150 on CMP 30HX/40HX (CMP 50HX: +225) unless `--uv-max` says otherwise. RTX 40/50 cards often hold +450..480.
   A saved result above a lowered limit is not used as is: the card runs at the limit (after a quick check) and the saved
   result is kept for when the limit goes up again; a saved result that *is* the old limit is tuned again when the limit rises.
4. The result is saved in `uv.json`, keyed by GPU UUID, driver version and kernel build. Later starts apply it after a
   quick check (about 30 s). If the driver, the kernel build or your starting offset changed, the GPU is tuned again.
   Each check runs in a child process that needs about 2.4 GB of RAM (`FFGP_UV_CHECK_MB`): on a rig with little RAM the
   checks of several GPUs run together only while the free RAM covers them, the others wait (`waits for RAM` in the log) and
   go next; mining starts when every GPU is tuned or applied. While mining, a re-check takes its RAM first and only then
   pauses its GPU (since 1.7.1), so a GPU that waits for RAM keeps mining.
5. While mining: every 6 hours (`--uv-reval-hours`) the setting is re-checked (mining on that GPU pauses for about 30 s).
   If a found result fails the CPU verification, or a burst of rejected shares appears, the offset goes one step down (never
   below your own value) and is saved. **OC applies (since 1.7.1):** HiveOS `nvidia-oc` writes the *Core Clock* value of the
   OC profile at every miner start and every OC change, **0 when that field is empty** — before 1.7.1 this switched the
   undervolt off until the next restart (a 4x RTX 3070 rig ran at 604 W instead of ~440 W). Now an offset that is read back as
   0, as the HiveOS profile value or as your own starting offset counts as an OC apply: auto-uv sets the verified offset again
   after a quick check (mining on that GPU pauses for ~30 s; a failure steps back as in the re-check; at most 4 times an hour),
   log `the offset was reset to +0 by an OC apply (...); re-applying the verified +285 after a quick check`. A check during
   which an OC apply reset the offset is repeated. Any other value is your own choice: auto-uv stands down on that GPU and
   does not fight.
6. On exit (Ctrl+C, SIGTERM, HiveOS `miner stop`, closing the Windows console, normal end) your offset is restored on every
   GPU (before 1.6.1 a `miner stop` on HiveOS could restore only GPU0). If the miner was killed (`kill -9`, crash), the next start
   restores your offset first, from the restore file `uv-restore.json`. A reboot resets the offset in the driver anyway.
   A stop while a check runs (HiveOS `miner stop` / `miner restart` sends Ctrl+C and then closes the screen session) is not a
   failure (since 1.7.1, also for CMFD and NOID): the offset under check is neither taken for a crash point nor paused for 24 h;
   the next start goes on as before.

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
     `set "UV_MAX="` (empty = per card: +495, RTX 20xx/GTX 16xx/CMP 50HX +225, CMP 30HX/40HX +150; RTX 40/50 often hold +450..480, RTX 30
     usually +200..360). A comma list sets one value per GPU in `DEVICES` order, e.g. `set "UV_MAX=225,495,150"`.
   * `start-nvidia-smi.bat`: `AUTO_UV=1`, the range in `UV_MIN` / `UV_MAX`, plus power limit and clock lock through `nvidia-smi`.
   * your own command line: `freeforgem.exe --pool ... --wallet ... --auto-uv --uv-max 480`.
4. **Starting point.** The tuner starts from the offset the card has now and never goes below it. The *Core Clock* offset of
   MSI Afterburner (or a similar tool) is this same offset, so a value you set there is your starting point and is put back
   on exit. If Afterburner re-applies a profile while mining (the offset goes back to your value or to 0), auto-uv puts the
   verified offset back after a quick check (since 1.7.1); any other value makes it stand down on that GPU. With no offset set the tuner starts at +120, on RTX 40 at +200, on RTX 50 at +300 (or `--uv-min` / `UV_MIN`).
5. **Wait.** The first start tunes for about 15 minutes per GPU (all GPUs in parallel) and only then starts mining. Later
   starts apply the saved value after a ~30 s check.
6. **Files** are in the miner folder: `uv.json` (results) and `uv-restore.json` (your original offset). The folder must be
   writable, so do not put the miner into `C:\Program Files`. To tune again: close the miner, delete `uv.json` (or run
   `freeforgem.exe --uv-reset` in that folder) and start again. A new driver or a new miner kernel triggers a new tuning by itself.
7. **Read the result:** `findstr auto-uv logs\*.log` (`start-nvidia-smi.bat`: `logs\ffgp.log`). Example from that RTX 3070:
```
auto-uv: GPU0 (bus 04, RTX 3070): CUDA device 0 -> NVML index 0, PCI 0000:04:00.0, ..., current offset +225 (allowed -1000..1000)
auto-uv: GPU0 (bus 04, RTX 3070) step +255: ok (6560 reruns, cpu ok 6), 94.0 W, 1560 MHz, 58 C
auto-uv: GPU0 (bus 04, RTX 3070) step +270: FAILED (rerun_diff: 256 transcript word(s) differ between reruns of seed 3 ...)
auto-uv: GPU0 (bus 04, RTX 3070) offset +225 -> +240, 98.4 W -> 86.5 W, saved
```
   * `auto-uv paused for this GPU for 24 h`: the card had an error at the found offset; it mines at your own offset and
     the tuner tries again after 24 h (or after `--uv-reset`).
   * `start offset ... leaves no room below the cap`: your offset is already at `UV_MAX` (or the card's default limit, shown
     in brackets) or higher; raise `UV_MAX` if you are sure the card takes more.
   * `no safe offset above ... was found`: the card does not take more than your own value; nothing is changed.
8. **If the PC hangs or the driver resets** (black screen, "display driver stopped responding"): reboot and start the miner
   again; it first restores your offset from `uv-restore.json`. If the miner itself crashed during a tuning step (window shows
   `Miner exited (code -1073741819)`), the next start counts that step as failed and tunes only below it (since 1.5.2; before,
   it started the whole search again and crashed at the same step). Since 1.5.3 it also skips the steps that already
   passed: after the warm-up it goes straight to the confirmation (about 5 minutes instead of a full search). If a card keeps failing, lower `UV_MAX` or leave `AUTO_UV` off.

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
  For the undervolt `AUTO_UV=1` alone is enough: the limits are chosen per card (RTX 20xx/GTX 16xx/CMP 50HX +225, CMP 30HX/40HX +150, others +495;
  RTX 40/50 start at +200/+300 by themselves). For manual control `UV_MIN` / `UV_MAX` / `UV_OFFSET` take a list in the same order,
  e.g. `set "UV_MAX=225,495,150"` (`-` = that GPU's default). A list must have exactly one value per GPU.
* **Faster tuning:** `set "UV_STEP=30"` halves the number of steps (coarser result). On RTX 50 a higher start (about +300)
  saves the most time: the card holds it anyway, and the search starts there (since 1.6.2 this is the default on RTX 50). Do not set it too high: if the card fails
  the check already at `UV_MIN`, it is not tuned and stays at your own offset (the log says so; lower `UV_MIN` then).
* **Already know your offset:** `set "UV_OFFSET=480"` (and `AUTO_UV` left off) applies it after one quick check, no search.
* **Power limit as well:** use `start-nvidia-smi.bat` (`CCLOCK` / `MCLOCK` / `PLIMIT`, `UV_MIN` / `UV_MAX` / `UV_STEP`).
* **Search again after changing the range:** add `set "EXTRA=--uv-reset"` for one start, then empty it again.

**Risk.** Undervolting beyond what a card can take makes a driver reset or a hang possible (the tuner only goes where it
has just checked, with a margin, but silicon, temperature and driver differ). After a hang, reboot: the next start restores your
offset first. Use it at your own risk, as with any overclocking.

**Enable**
* HiveOS: put `--auto-uv` into *Extra config arguments* of the flight sheet (details in HIVEOS.md).
* Linux: `sudo ./freeforgem --pool ... --wallet ... --auto-uv`
* Windows: right-click `start.bat` → *Edit* and remove `rem` before `set "AUTO_UV=1"`; start `start.bat` **as Administrator**.

| Option | Meaning |
|---|---|
| `--auto-uv` | find, apply and supervise the undervolt (as described above) |
| `--uv-offset N` | apply the fixed offset N MHz after a quick check, no tuning (if the check fails, your offset stays) |
| `--uv-reset` | forget the saved results; with `--auto-uv` the GPUs are tuned again; without a wallet the miner just exits |
| `--uv-max N` | highest offset the tuner tries (at most 600; default per card: RTX 20xx / GTX 16xx / CMP 50HX 225, CMP 30HX/40HX 150, all others 495; on Windows: `set "UV_MAX=480"` in `start.bat`) |
| `--uv-min N` | offset the tuner starts from and never goes below (default: the HiveOS/driver offset, else 120; RTX 40: at least 200, RTX 50: at least 300; Windows: `UV_MIN`) |
| `--uv-step N` | MHz per tuning step (default 15; Windows: `UV_STEP`) |
| `--uv-keep-last` | after a failed step keep the last good step (default: the last good step minus one step) |
| `--uv-reval-hours H` | re-validation interval while mining (default 6, `0` = never) |
| `--uv-dir DIR` | where `uv.json` and `uv-restore.json` are kept (also `FFGP_STATE_DIR`) |
| `--auto-uv-dry-run` | only logs which GPUs would be changed (CUDA device → NVML device, bus id) and their ranges; changes nothing |

`--uv-max`, `--uv-min` and `--uv-offset` take one value for all GPUs, or a comma list with exactly one value per GPU in
`--devices` order (without `--devices`: GPU0, GPU1, ... as in the log; on HiveOS that is the PCI bus order of the dashboard).
`-` in a list = that GPU's default (`--uv-offset`: the GPU is left alone). A list of the wrong length is an error, it is never
extended. An explicit `--uv-max` is used as given, also above the card's default; the log then warns
(`WARNING --uv-max +495 is above the recommended +150 for this card (CMP default)`). Example for 2× RTX 2060 SUPER,
3× RTX 3070 Laptop and a CMP 40HX: `--auto-uv --uv-max 225,225,495,495,495,150` (the same as the defaults).

**GPU clock control (optional, NVML; needs root / Administrator, works on Windows — new in 1.4.9).** `--lock-cclock N` locks the core clock to N MHz (like `nvidia-smi -lgc N,N`), `--lock-mclock N` locks the memory clock to N MHz (like `-lmc`), and `--moffset N` sets a memory-clock offset in MHz, which may be negative (needs driver 555+). Each takes one value for all GPUs, or a comma-separated list in `--devices` order (one value per device). The settings are applied at start and undone on exit (Ctrl-C / SIGTERM / crash). **Since 1.7.1 a lock that was there before is put back instead of removed:** on HiveOS the lock of the OC profile (*Lock Core Clock* / *Lock Memory Clock*, read from `/hive-config/nvidia-oc.conf`; before 1.7.1 a test run with `--lock-cclock` left a rig without its HiveOS lock); elsewhere NVML cannot read a lock back, so name it in `FFG_RESTORE_CCLOCK` / `FFG_RESTORE_MCLOCK` (the FreeForGemPear names `FFGP_RESTORE_*` work too; one value or a list per GPU, 0 = none), otherwise the clock is reset to default as before. The log says what happens: `core clock locked to 1500 MHz; lock before: 1470 MHz (HiveOS OC profile), put back on exit`, at exit `core clock lock 1470 MHz put back`. After a crash or `kill -9` the next start does the same. On Windows set `LOCK_CCLOCK`, `LOCK_MCLOCK`, `MOFFSET` in the `.bat` file. Example: `--lock-cclock 1500 --lock-mclock 810 --moffset -200`.

Default location of the files: next to the binary (Windows: the miner folder), on HiveOS `/hive-config/freeforgem`
(survives miner updates and reboots). **Reset:** `freeforgem --uv-reset` (or delete `uv.json`).

**Log lines** (all start with `auto-uv:`, so `grep auto-uv` shows the story):
```
auto-uv: GPU0 (bus 0d, RTX 3070): CUDA device 0 -> NVML index 1, PCI 0000:0D:00.0, GPU-0d06..., current offset +150
auto-uv: GPU0 (bus 0d, RTX 3070): range +150..+495 (max: default; start: current offset)
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
   HiveOS messages must show no `Xid` (13, 31, 43, 79 ...). `freeforgem --uv-offset N` applies N, runs the same check as
   `--auto-uv` and mines with it, which is a quick way to test a candidate value.
4. Repeat from step 2 until the first problem, then go back **two steps (30 MHz)** from the offset where it appeared and keep that.
5. Test again after a driver update, a new cooler or hot weather.

### 13. Common Foundry (CMFD), new in 1.6.0

The same binary also mines **Common Foundry (CMFD, ForgeMatrix V4)**. The CMFD work is done by the
**FreeForgeMiner** engine (MIT, <https://github.com/pepsykolya/freeforgeminer>, release 1.2.4), in the `cmfd/` folder of the
package: Windows uses the release binaries unchanged; the Linux/HiveOS binaries are built from the same pinned source on
Ubuntu 20.04, so they run on every HiveOS (Ubuntu 20.04 and 22.04 images). FreeForGem prepares the model, runs one engine process per GPU, applies your clock
settings, prints its usual stats line and restarts an engine that exits.

**The coin follows the pool URL**: a `cmfd+tls://` URL means CMFD (`--algo cmfd|pearl` forces it). The wallet is your
CMFD address in either form: `cmfd1...` (63 characters with a checksum, as the Common Foundry wallet
1.0.16+ shows it) or the older 64 hexadecimal characters; both are the same key. A `cmfd1...` address is checked
(a typo is refused before mining starts) and sent to the pool as hex. Test-network `tcmfd1...` addresses are refused.

**Requirements.** NVIDIA RTX 20xx or newer (tuned for RTX 30/40), about **7 GB of free GPU memory per card** (the model alone
is 6.4 GB), NVIDIA driver R575+ on Linux/HiveOS, 7 GB of disk for the model. Linux/HiveOS: glibc 2.30+ (Ubuntu 20.04 or newer, every
HiveOS image).

**The model** (6.4 GB) is downloaded once from the official Common Foundry servers in authenticated parts and then only
verified at each start (takes 10–40 s). Location: `--cmfd-model DIR`; default `/hive/miners/custom/cmfd-model` on HiveOS
(shared with FreeForgeMiner, so a rig that already ran it does not download again), otherwise `cmfd-model` next to the binary.

**Pools** (URL with the certificate pin, exactly as below; operator fee in brackets):

| Pool | URL |
|---|---|
| **Aria / AriaBrain** (default, pool.ariabrain.com/cmfd.html, 3 %) | `cmfd+tls://159.69.194.46:29445?pin=9dfb51083f287726117f689f87bc7a878792efcca58ca8b6f6e7c05ac5d152e9` |
| **Midas Mining** (midasmining.net, 1 %) | `cmfd+tls://24.107.152.151:29745?pin=2ee4e6a028b07b300753dff3c260b79763c5e748d4e78b86cf3855e97eb2e80e` |
| cmfd-pool.online (3 %) | `cmfd+tls://109.199.124.187:29465?pin=ebe88f5e05f3a222208d551d05b6d39057b64ce8239ba7a708d487e15ac711be` |

A pool publishes its URL and pin itself (Midas: `https://midasmining.net/api/v1/pool`, field `public_pool_url`); a changed pin
means a changed certificate: take the new line from the pool, never disable the pin. Some pools (cmfd-pool.online) ban the
**IP address** for about an hour after a flood of rejected shares or reconnects: do not test unstable clocks on them.

**Linux:**
```
./freeforgem --pool 'cmfd+tls://159.69.194.46:29445?pin=9dfb51083f287726117f689f87bc7a878792efcca58ca8b6f6e7c05ac5d152e9' \
    --wallet cmfd1YOUR_ADDRESS --worker rig1
# Midas Mining:
./freeforgem --pool 'cmfd+tls://24.107.152.151:29745?pin=2ee4e6a028b07b300753dff3c260b79763c5e748d4e78b86cf3855e97eb2e80e' \
    --wallet cmfd1YOUR_ADDRESS --worker rig1
./freeforgem --algo cmfd --self-test        # GPU check, no pool: exact comparison with the reference path; with the model
                                                # also the official Common Foundry 1.0.16 test vector (32 nonces, SHA-256)
```
Quote the URL in a shell (`?` is a wildcard). Run as root if the miner should reset an undervolt offset (see below).

**HiveOS** (flight sheet, Miner *Custom*, same package as for Pearl): *Hash algorithm* `forgematrix_v4`, *Wallet and worker
template* `%WAL%.%WORKER_NAME%` with your CMFD address as the wallet, *Pool URL* one line from the table, e.g.
`cmfd+tls://159.69.194.46:29445?pin=9dfb51083f287726117f689f87bc7a878792efcca58ca8b6f6e7c05ac5d152e9`
(Midas: `cmfd+tls://24.107.152.151:29745?pin=2ee4e6a028b07b300753dff3c260b79763c5e748d4e78b86cf3855e97eb2e80e`).
*Extra config* (optional): `--cmfd-mode eco`, `--devices 0,1`, `--per-gpu-workers`, `--lock-cclock 1560`.
HiveOS shows FW/s per card; the first start of a rig downloads the model (watch the miner log).
Give the CMFD flight sheet **its own OC profile without the memory lock** (see *Memory clock* below): the Pearl OC
cuts CMFD about 6×.

**Windows:** `start-cmfd.bat` — set `WALLET`, keep `POOL` (Aria) or remove `rem` from the Midas line (or another one),
optionally `DEVICES`, `MODE`, clock locks. Run it as Administrator if the card has an undervolt offset.

**Options:** `--cmfd-mode speed|eco` (speed default; eco: RTX 30 about 13 % fewer watts for 2 % less hashrate, RTX 40/50 the
same in both), `--cmfd-batch N` (1–64, default by GPU memory: 64 on 11 GB+, else 32), `--cmfd-model DIR`, `--cmfd-dir DIR`
(engine folder), `--per-gpu-workers` (`WORKER.gpuN` on the pool instead of one worker per rig), `--lock-cclock/--lock-mclock/
--moffset` as in section 7, `--devices`, `--minutes`.

**Clocks and undervolt.** CMFD loads the tensor cores much harder than Pearl. A core V/F **offset** that is fine for Pearl
(+200 on an RTX 3070, +330 on an RTX 4070 Ti) makes CMFD fail at once ("illegal memory access", "misaligned address") or
return wrong results. Therefore in CMFD mode the miner **sets a positive core offset to 0** before the engine starts and puts
the old value back on exit (log: `cmfd offset: GPU0: core offset +345 -> 0 MHz ...`; after a crash the next start puts it
back first). This needs root/Administrator; without it the log says so and you must remove the offset yourself (HiveOS:
the OC profile of the CMFD flight sheet). `--uv-offset N` sets N instead — **at your own risk**. On **AMD Radeon** `--auto-uv` tunes an undervolt
for CMFD with amduv (section 14, since 1.7.1); on NVIDIA see below.
Safe settings: stock clocks, a power limit, or a core lock (`--lock-cclock`).

**`--auto-uv` for CMFD (NVIDIA).** With `--auto-uv` (HiveOS: *Extra config arguments*; Windows: `AUTO_UV=1` in
`start-cmfd.bat`, run as Administrator) the miner finds an undervolt **for CMFD itself**, separate from the Pearl result.
After the model is prepared and before the engine starts, per card: the engine's own GPU worker (`cmfd-v4-replay`, unchanged)
loads the model once and runs two fixed batches again and again; at the base offset (0, stock) it takes their final digests as
the reference and runs the official Common Foundry 1.0.16 test vector; then the core offset goes up in `--uv-step` steps
(15 MHz) from `--uv-min` (default: one step above 0) up to `--uv-max` (default: the per-card limit of section 12: 495,
RTX 20xx / CMP 50HX 225, CMP 30HX/40HX 150); every step runs 30 s + 10 s of measurement and **every digest must be
bit-identical** to the reference (a single wrong bit anywhere in the 384 layers changes it). The first failing step is the
edge; the result is one step lower (`--uv-keep-last` keeps the last good one), confirmed for 2 minutes + the test vector
(one more step down on failure). It is saved in `uv.json` (section `cmfd`, per card UUID, with the driver and the engine/worker
identity) and later starts apply it at once (or after a 30 s check when it was verified more than `--uv-reval-hours` ago).
While mining, an engine `INTEGRITY | ERROR` or a CUDA fault on a tuned card steps its offset back one step (saved) and
restarts the engine. An OC apply that resets the tuned offset while mining (HiveOS `nvidia-oc` at a miner start or after an
OC change: the *Core Clock* value of the profile, 0 when the field is empty) gets it back (since 1.7.1): the engine of that GPU
stops, the saved offset gets the 30 s quick check on the model + the test vector (a failure steps back), and the engine
restarts at it; at most 4 times an hour, any other value makes the tuner stand down on that GPU. A check during which an OC
apply reset the offset is repeated. Your own offset comes back on exit (and after a crash at the next start), as without
`--auto-uv`.
If the check cannot run (no model, worker missing, failure at the base offset) the card mines at 0 with a WARNING.
Tuning takes about 2 + 0.75 × steps minutes per card (all cards at once). Log: `grep "auto-uv (cmfd)"`, e.g.
`auto-uv (cmfd): GPU0 (bus 0d, RTX 3070) CMFD offset +0 -> +135: 19.53 -> 20.94 FW/s, 210.7 -> 215.8 W, ... saved`.
Measured (Aria, averages of the stats line): RTX 3070 with a 135 W power limit is power-bound at ~1050 MHz, where the voltage
is already at the bottom of the curve: edge +225, result +195, no gain (13.84 → 13.97 FW/s at 135 W). The same card at 220 W:
stock 19.5 FW/s at 214 W (328 k FW/kWh) → +135 (edge +165) 20.4–20.5 FW/s at 213–216 W (342–345 k, **+4–5 %**).
CMP 50HX (power limit 225 W, core locked at 1410 MHz): no edge up to its +225 limit; 13.10 FW/s at 216 W → 13.21 at 196 W
(**+11 %**), 13.01 at 217 W → 13.17 at 203 W (**+8 %**). **The edge depends on the operating point**: the result is tuned again
when the power limit changes by more than 3 % or `--lock-cclock` changes; after changing the core clock lock of the OC profile
(HiveOS, nvidia-smi) run once with `--uv-reset`.

**Memory clock: no memory lock for CMFD.** Pearl hardly needs the memory, CMFD does. The usual Pearl OC with the memory
clock locked low (HiveOS OC *Mem lock* 810, `nvidia-smi -lmc 810`, `--lock-mclock 810`) cuts CMFD about **6×**: on a HiveOS
rig with 4× RTX 3070 one card gave **3.7 FW/s** with the memory locked at 810 MHz and **21.8 FW/s** (~237 W, power limit
240 W) with the memory free. Use a **separate OC profile for CMFD**: no memory lock, core offset 0, a power limit as you
like. Since 1.6.1 the miner checks this once per GPU after 30 s of mining and warns
(`[GPU0] WARNING: memory clock 810 MHz of 7001 MHz while mining CMFD ...`); it never changes your OC itself.

**Log.** The engine's lines are shown with a `[GPUn]` prefix (`MINER STATS`, `INTEGRITY | ok`, `SHARE STALE` = harmless, a new
block arrived in flight; `SHARE REJECTED` = wrong result, lower clocks). Every 10 s FreeForGem prints
`[ 290s] GPU0 37.04 (190 W, 701390 FW/kWh) | total 37.04 FW/s | ... | user sub 84 acc 80 rej 0 | stale 4 | height 25089`
(`[DEVFEE]` during the fee slice; `STARTING` while the engine loads the model; `RESTARTING`, `DISABLED`).
An engine that exits is restarted after 15/30/60/120/300 s; after 5 failed starts in a row (each under 2 minutes) that GPU
is left alone and the log says why (no endless reconnects that could get the IP banned).

**Dev fee for CMFD: 1 %**, inside the engine: 36 seconds per hour per GPU, random slot, on the pool you selected, worker
`ffm-fee`, log lines `DEV FEE | ...`. The Pearl fee of section 10 is not run in CMFD mode (total 1 %).

**Hashrate** (measured on the Aria pool, 5–30-minute runs, offset 0, auto batch): RTX 4070 Ti (power limit 190 W)
36.8 FW/s at 187 W; RTX 3070 (power limit 135 W) 14.1 FW/s at 135 W; same as FreeForgeMiner 1.2.3 on the same cards
(36.8 / 14.2 FW/s). With the default power limit an RTX 3070 does about 20 FW/s (HiveOS, power limit 240 W, memory not
locked: 21.8 FW/s at ~237 W).

**Troubleshooting:** `out of memory` — close programs using the GPU, CMFD needs ~7 GB free; `illegal memory access` /
`misaligned address` / `INTEGRITY | ERROR` — remove the core offset, lower clocks; `model preparation ... failed` — check
internet access to downloads.commonfoundry.ai and github.com, free disk space; a pool refusing the URL — take its current
line (pin) from the pool's site.


### 14. AMD Radeon (Windows), new in 1.6.0

The Windows package also mines on **AMD Radeon** cards with its own OpenCL kernel (`pearlgpu_amd.dll`, closed source like
the CUDA one). Same binary, same start files, same log; the miner picks the GPU library by itself:

* `--backend auto` (default): the CUDA library if it sees an NVIDIA GPU, otherwise the AMD library. `--backend amd` /
  `--backend cuda` (or `FFGP_BACKEND`) force it. The log shows the choice: `1 GPU(s) [amd]` and
  `[pearlgpu-amd] gfx1030: amd-a0 udot4 k8192`.
* Only **discrete** Radeon cards are used; the integrated "AMD Radeon(TM) Graphics" of Ryzen CPUs is skipped.
* One process drives one vendor: on a PC with NVIDIA and AMD cards run two miners (`--backend cuda` and `--backend amd`).

| AMD GPU | Status |
|---|---|
| RX 6000 (RDNA2, e.g. RX 6800 XT) | **tested**: Pearl 34.4 TH/s at ~260 W (stock), self-test, pool shares; CMFD ~10 FW/s |
| RX 7000 (RDNA3), RX 9000 (RDNA4) | should run the same kernel, **not tested** |
| RX 5000 (RDNA1), Vega and older, integrated graphics | not supported |
| Linux / HiveOS with AMD | **not available yet** (the Linux and HiveOS packages are NVIDIA-only) |

**Needs:** Windows 10/11 x64 and the AMD Adrenalin driver (it brings OpenCL); no NVIDIA driver. Run any `start*.bat` as
for NVIDIA. Self-test: `freeforgem.exe --self-test` (checks the AMD kernel against the CPU reference, `== self-test PASSED ==`).

**Busy CPU:** with AMD the miner waits for the GPU actively and runs at Above Normal priority, so a CPU miner or other load
on all cores does not starve the GPU (33.7–33.9 instead of 32.6 TH/s next to a CPU miner on all cores). This keeps one CPU
thread busy; `PEARL_AMD_SPIN=0` / `FFGP_PROC_PRIO=0` turn it off.

**Watts** in the stats line come from the driver (ADLX, chip power, read-only). On a rig with several AMD cards the per-card
watts may be swapped between cards (the rig total is right).

**Undervolt (`--auto-uv`) on AMD** works differently from NVIDIA: the miner starts the `amduv.exe` tool next to it, which
tunes the core clock limit at the lowest core voltage (every point is checked bit-for-bit on the GPU and the CPU), once per
card / driver / AMD library version (~10–15 min), and then applies the result at each start. A guard process puts **your own
clock / voltage / power-limit settings back** when the miner exits, also after a crash or a kill. Run as Administrator.
Result on our RX 6800 XT: about 30–32 TH/s at 160–172 W instead of 34 TH/s at ~260 W (+40 % TH/W). `$FFGP_AMD_UV_GOAL=max`
keeps more speed; `--uv-reset` forgets the result. `--uv-offset`, `--lock-cclock`, `--lock-mclock` and `--moffset` are
NVIDIA-only (the miner refuses them with AMD).

**CMFD on AMD:** `start-cmfd.bat` / `--pool cmfd+tls://…` works the same way; the package runs its OpenCL worker
`cmfd\cmfd-v4-replay-ocl.exe` and the pool client `cmfd\cmfd-miner-ffgp.exe` (FreeForgeMiner rebuilt with the small patch in
`cmfd/patches/`, so it runs without nvidia-smi). Needs ~7 GB of free GPU memory. RX 6800 XT: ~10 FW/s at ~275 W (an RTX 3070
does ~20 FW/s): CMFD is much better on NVIDIA tensor cores.

**Undervolt for CMFD on AMD (since 1.7.1):** remove `rem` before `set "AUTO_UV=1"` in `start-cmfd.bat` (run it as
Administrator), or add `--auto-uv` to a CMFD command line. `amduv.exe` keeps a **separate CMFD result**: CMFD computes wrongly
at clocks where Pearl is still fine (RX 6800 XT at the lowest voltage: Pearl passes up to 2250 MHz, CMFD fails from 2200–2250),
so the Pearl point is never used for CMFD. Every point is checked on the real model: the OpenCL worker runs fixed batches of 32
nonces again and again, and their final digests (BLAKE3 over all 384 layers) must equal bit for bit the ones taken at stock —
one wrong bit anywhere changes them. The first start tunes (~15–20 min, once per card / driver / worker / model), later starts
apply the saved point (a 15-second check when the last one is older than 6 h). The guard puts **your own clock / voltage /
power-limit settings back** when the miner exits, also after a crash or a kill. `UV_GOAL` in `start-cmfd.bat`
(`$FFGP_AMD_UV_GOAL`) picks one of the tuned profiles:

| Profile | How it is found | RX 6800 XT (ASIC watts, ADLX) |
|---|---|---|
| — (stock) | | 10.0 FW/s at 280 W (36 FW/kJ) |
| `eff` | lowest voltage, max clock raised until the check fails, one step back | 2100 MHz / 881 mV: 9.1 FW/s at 193 W (47 FW/kJ, **+32 %**; −9 % FW/s) |
| `bal` (default) | the most FW per watt that keeps ≥ 90 % of the stock FW/s | = `eff` on this card |
| `max` | your max clock, voltage lowered until the check fails, one step back up | 2324 MHz / 1075 mV: 10.1 FW/s at 248 W (41 FW/kJ, +14 %; same speed) |

On the Aria pool through the miner (10–15 min each, average of the stats line): stock 9.9 FW/s at 277 W, `bal` 9.1 FW/s at
191 W (+33 % FW/W), `max` 10.0 FW/s at 247 W (+14 %); no rejected shares, engine `INTEGRITY | ok`. The edge moves a little from
run to run (2200 MHz at 881 mV passed twice and failed once), so the tuner always keeps one step of margin.

Not useful on Windows: the power limit goes down only to −6 % (264 W, +4 % FW/W); the memory clock can only be raised
(2000–2150 MHz: no gain, and wrong results at 2150), the fast memory timing changes nothing. `--uv-reset` forgets the Pearl
and CMFD results (the next `--auto-uv` start tunes again).

### 15. NOID (Parano1d), new in 1.7.0

FreeForGem mines **NOID** (Parano1d: Poseidon2b over GF(2^128), proof of work) on NVIDIA GPUs with its own kernels (closed
source, `libnoidgpu.so` / `noidgpu.dll`, separate from the Pearl library). The dev fee is **1%** (one 60 s slot in every
100 minutes, on your pool), like Pearl. Every candidate the GPU reports is computed again on the CPU with the official
`noid_poseidon2b` crate before it is submitted.

**Requirements**

* NVIDIA **RTX 30 / RTX 40 / RTX 50** or A100/H100 (compute capability 8.0+). RTX 20 / GTX 16 / CMP (Turing) and AMD are
  **not** supported for NOID.
* NVIDIA driver **580 or newer** (the kernels use the `clmad` instruction of CUDA 13). With an older driver the miner stops with
  `NOID needs an NVIDIA driver R580 or newer`. HiveOS: `nvidia-driver-update 580` (or newer).
* A NOID wallet address (`o1...`) and a NOID pool. NOID pools speak `parano1d-stratum-v1` over **TLS**:

| Pool | URL | Note |
|---|---|---|
| InnovLab (EU) | `stratum+ssl://eu2.innovlab.cc:19601` | tested with FreeForGem (shares accepted) |
| Suprnova | `stratum+ssl://noid.suprnova.cc:3340` | TLS handshake checked, not mined |

A bare `HOST:PORT` is taken as TLS; `stratum+tcp://` is plain TCP; `?insecure` at the end of the URL accepts a self-signed
pool certificate. The coin is chosen by `--algo noid` (or automatically when the pool host name contains `noid`, `parano1d`
or `innovlab`). The pool login is `WALLET.WORKER`.

**Two kernels: `--noid-mode eff` (default) and `--noid-mode speed`**

RTX 3070, memory 810 MHz, power limit 135 W, 60 s per point (`eff` = 512 threads per SM, `speed` = more of the S-box on CLMAD;
both use the shared-operand S-box chain x·(x·x²)²):

| Core clock / offset | eff: MH/s @ W (MH/W) | speed: MH/s @ W (MH/W) |
|---|---|---|
| 1400 MHz / +220 | **34.70 @ 73.2 (0.474)** | — |
| 1560 MHz / +205 | 38.40 @ 92.2 (0.417) (offset +175) | 43.65 @ 105.3 (0.415) |
| 1650 MHz / +220 | — | **46.16 @ 121.1 (0.381)** |

* **eff** (default, best MH per watt): lock the core at **1400 MHz** and use `--auto-uv`.
* **speed** (best MH/s): core **1560–1650 MHz** (RTX 30): 43.65 MH/s at 1560 MHz, 46.16 MH/s at 1650 MHz.
* RTX 40/50: the same kernels built for sm_89 / sm_120; tune the clock yourself (start at the card's stock clock with `--auto-uv`).

**Command line**

```
./freeforgem --algo noid --pool stratum+ssl://eu2.innovlab.cc:19601 --wallet o1YOUR_ADDRESS --worker rig1 \
             --noid-mode eff --lock-cclock 1400 --lock-mclock 810 --auto-uv
./freeforgem --algo noid --self-test          # official test vectors + GPU vs CPU digests, both kernels, every GPU
```

**HiveOS flight sheet:** Coin `NOID` (any name), Wallet `o1...`, Pool: Configure in miner; Miner **Custom** → Miner name
`freeforgem`, Installation URL of the release, Hash algorithm `parano1d`, Wallet and worker template `%WAL%.%WORKER_NAME%`,
Pool URL `stratum+ssl://eu2.innovlab.cc:19601`, Extra config `--algo noid --lock-cclock 1400 --auto-uv`. The dashboard shows
the hashrate in kH/s (MH/s × 1000).

**Windows:** edit `start-noid.bat` (WALLET, POOL, MODE, optional AUTO_UV=1 and LOCK_CCLOCK; run as Administrator for
AUTO_UV/clocks) and double-click it.

**Undervolt for NOID.** NOID needs its own offset: a Pearl undervolt is usually too deep for it (RTX 3070: Pearl +250 gave
wrong NOID results; RTX 4070 Ti: the owner's Pearl +300 crashes NOID). So:

* without `--auto-uv`, a **positive core offset is set to 0** for the NOID run and put back on exit;
* with `--auto-uv`, FreeForGem tunes a NOID offset per card **and per mode** (section 16 explains why every step takes 40 s):
  base offset checked, then from `+120` (`--uv-min`) up in 15 MHz steps (`--uv-step`) to the card's limit (`--uv-max`, the same
  per-card defaults as Pearl); the first failing step is the edge, the result is one step below the last good step, confirmed
  for 2 minutes. About 10 minutes on the first start; later starts apply the saved result (a 30 s check when it is older than
  `--uv-reval-hours`). The result is saved in `uv.json` under `noid` (the Pearl and CMFD results are kept), together with the
  driver, the kernel, the core clock lock and the power limit: if any of them changes, the card is tuned again;
* `--uv-offset N` sets N without checks (at your own risk); `--uv-reset` forgets the NOID results;
* while mining, every candidate is re-checked on the CPU, and every 120 s a short watchdog check (6 launches of the fixed check
  job, bit-exact) runs on each GPU; a failure steps that card's offset back by one step (saved) and mining goes on;
* an OC apply while mining (HiveOS `nvidia-oc` at a miner start or after an OC change writes the profile's *Core Clock* value,
  0 when the field is empty; a Pearl offset there gives silently wrong NOID results): the offset is read back every 30 s, a
  tuned NOID offset comes back after a 30 s stress check with mining on that GPU paused (a failure steps back), the base 0
  (without `--auto-uv`) or the `--uv-offset` value is set again at once; at most 4 times an hour, any other value makes the
  miner stand down on that GPU (since 1.7.1). A check during which an OC apply reset the offset is repeated.
* RTX 3070, our runs: **eff @1400 MHz** — 87.5 W at offset 0 → **72.8 W at +195** (34.3 MH/s; the voltage floor: above +195
  the power no longer drops, so the tuner stops there), MH/W **+20 %**; on the pool 34.8 MH/s @ 73 W (0.476 MH/W), 41/41 shares
  accepted. **speed @1560 MHz** — 127.8 W → **106.2 W at +195** (43.3 MH/s), edge +225, MH/W **+20 %**; on the pool
  43.8 MH/s @ 107 W (0.410), 24/24 accepted. RTX 4070 Ti at stock clocks and power limit 190 W, offset 0: speed 82.6 MH/s,
  eff ~81 MH/s (both at the power limit; not tuned yet).

**Log (NOID)**

```
FreeForGem 1.7.2-beta | NOID (parano1d) | pool stratum+ssl://eu2.innovlab.cc:19601 | worker rig1 | 1 GPU(s) [0] | mode eff
[noid-pool:user] subscribed: parano1d-stratum-v1, nonce_bits 64, session namespace 24e67d0000000200
new job 46f6...0000 [user] height 239348 (share target 0000003fffffffff.., expires in 262s)
[  60s] GPU0 34.700 (73 W, 0.474 MH/W) | total 34.700 MH/s, 73 W, 0.474 MH/W | hits 412 (bad 0, stale 0) | user sub 3 acc 3 rej 0 | fee sub 0 acc 0 rej 0
```

`hits` = candidates re-checked on the CPU (the kernel reports more than the shares, down to about 1 in 4 million nonces);
`bad` = candidates or watchdog launches that failed the check (wrong GPU results: the offset steps back); `stale` = shares of
a job the pool had already replaced (not sent). `pool pause (canonical-tip-unverified)` is normal: the pool drops the current
job and sends the next one at once.

### 16. The silent edge of undervolt (why `--auto-uv` checks so much)

When the core offset is raised, the GPU gets a lower voltage at the same clock. Far below the limit everything is fine. Close
to the limit the card does **not** crash, the driver reports **no error (no Xid)** and a quick self-test still **passes** —
but some results are already **wrong**. On NOID our measurement on an RTX 3070 at 1560 MHz: at +250, **168 of 5984** full
launches returned a wrong hit set, the first after about 12 s of load, without a single driver error; the 2-minute self-test
passed. One step lower (+235) still failed; +220 was clean.

What it costs: wrong results become **invalid shares** or **shares that are never found**. Neither is visible in a normal
miner log — the hashrate looks the same, only the pool pays less. A miner that only watches for crashes and Xid errors tunes
straight into this zone.

How FreeForGem handles it (Pearl, CMFD and NOID follow the same rule):

1. **Every step is checked bit-exactly under full load, many times** — not by a single self-test. Pearl: thousands of attempts
   of a fixed job, every tile digest compared with the first run and found results verified on the CPU with the official
   verifier. NOID: the long stress check — full-grid launches for 40 s per step, every launch's candidate set compared with the
   CPU set of the official crate (about 4000 launches per step on an RTX 3070). CMFD: digests of fixed batches on the real
   6.4 GB model compared with the stock reference (CMFD runs at offset 0 in 1.7.2-beta; the CMFD tuner ships separately).
2. **The first failing step marks the edge.** The result is **one step lower** (15 MHz margin; `--uv-keep-last` keeps the last
   good step) and is **confirmed** by a 2-minute run; if the confirmation fails, one more step down.
3. **While mining, results keep being checked:** every Pearl hit and every NOID candidate is recomputed on the CPU before it
   is submitted; NOID also runs a watchdog check every 120 s; Pearl re-validates every 6 hours. A failed check **steps the
   offset back automatically** (saved) and mining goes on.
4. The owner's offset is written to a restore file before the first change and **put back on exit** (and by the next start
   after a crash).

What you see in the log (NOID speed kernel, RTX 3070, `--lock-cclock 1560 --auto-uv`, our run; the last two lines are from a run with a deliberately too deep `--uv-offset 280`):

```
auto-uv (noid): GPU0 (bus 0d, RTX 3070) tuning the NOID offset (speed kernel speed:aff/sm86): base +0, start +120, limit +495 (default), step 15, 40 s of stress per step
auto-uv (noid): GPU0 (bus 0d, RTX 3070) base +0: ok (3448 launches checked, 43.29 MH/s, 127.8 W, 0.339 MH/W, 1560 MHz, 56 C)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) +120: ok (4597 launches checked, 43.30 MH/s, 114.4 W, 0.378 MH/W, 1560 MHz, 56 C)
...
auto-uv (noid): GPU0 (bus 0d, RTX 3070) +210: ok (4598 launches checked, 43.30 MH/s, 103.8 W, 0.417 MH/W, 1560 MHz, 55 C)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) +225: FAILED (WRONG RESULTS: 5 of 4597 launches gave a wrong candidate set, first after 14.1s (no Xid, no crash: the silent edge)): edge found
auto-uv (noid): GPU0 (bus 0d, RTX 3070) confirmation +195: ok (13790 launches checked, 43.30 MH/s, 106.2 W, 0.408 MH/W, 1560 MHz, 57 C)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) RESULT speed: offset +195 (edge +225, one step below the last good one); MH/W 0.339 -> 0.408 (+20.3%); saved to .../uv.json
...
GPU0: WATCHDOG: 1 of 6 check launches gave WRONG results (silent undervolt edge?)
auto-uv (noid): GPU0: wrong GPU results at offset +280 (CPU check of a hit / watchdog): stepped back to +265 and mining on
```

### 17. Profiles per clock lock, mining during tuning, crashes while mining (new in 1.7.2)

**Why.** The undervolt edge moves with the core clock: an offset verified at 1410 MHz is not safe at 1805 MHz. On an 8x RTX 3070
HiveOS rig another miner left a 1400 MHz lock behind, auto-uv found +450..+495 there, and after the HiveOS 1805 MHz lock came back
these offsets were "applied without a start check": illegal memory access, Xid 13/43, a restart loop. 1.7.2 changes this:

**One profile per lock.** auto-uv keeps one result per GPU, per coin / NOID mode and per core clock lock (`uv.json`, sections
`profiles`, `noid`, `cmfd_profiles`; the memory lock is part of the key when the miner sets it, e.g. `1805/mem810`):

* with `--lock-cclock N` the lock is known and picks its profile; switching 1805 -> 1410 -> 1805 reuses each lock's verified result
  at once (applied without a check when verified within `--uv-reval-hours`, else after the usual quick check); a lock without a
  profile is tuned, and the profiles of the other locks stay as they are;
* without `--lock-cclock` the miner cannot read the lock (NVML has no call for it; HiveOS or another miner may have changed it):
  mining starts at your offset, the clock under mining load (~20 s) picks the profile: `1805`, `~1815` (a lock set outside the
  miner) or `unlocked` (the clock floated during the tuning); none fits: the GPU is tuned for these clocks. NOID and CMFD measure
  the clock with a short check at the base offset;
* the start log lists the profiles of each GPU: `profiles: 1400:+210, 1900:+180; using 1400 for the core clock lock 1400 MHz`;
* `--uv-reset` forgets only the profile of the clocks (and coin / mode) of this run, the others stay; `--uv-reset-all` forgets all;
* a result from before 1.7.2 (one per GPU, no lock recorded) becomes the profile of the clock it was found at under load
  (`~1410`) and is used only after a check at the clocks of the run (CMFD: the lock in its engine identity becomes its key);
  NOID results are kept only when they were tuned with `--lock-cclock`;
* while mining, a rise of the load clock at the same offset (a lock raised under the miner) puts your offset back and lets the clock
  pick the profile again; after an OC apply without `--lock-cclock` the clock decides first, too.

**Mining during tuning.** Before 1.7.2 no GPU mined until every GPU was tuned (~25 min on the 8-GPU rig). Now mining starts at
once; each GPU's checks and tuning run in the background and pause only that GPU (for its tuning or its quick check); the
other GPUs mine meanwhile. The RAM
queue of the checks ("waits for RAM") lets the waiting GPU mine meanwhile.

**Your offset.** On HiveOS the offset put back on exit is the OC profile's *Core Clock* value of that GPU (`/hive-config/nvidia-oc.conf`),
not whatever the card had at start (Rig01: +325 left by an interrupted ffgs instead of the profile's +275); at start such a leftover
is replaced by the profile value. A result verified below your offset at these clocks is **used instead of it** (your offset is
past the edge there); it is never kept unchecked. A tuning whose start offset itself fails looks for a lower one that passes
(60 MHz steps) and keeps it as the profile. `WARNING`: a single `--lock-cclock` value for all GPUs replaces per-GPU HiveOS locks.

**Which GPU failed.** A CUDA fault on one GPU (Xid 13 "Out Of Range Address", Xid 43, "an illegal memory access") ends CUDA for
the **whole miner process**: every worker stops with the same error, also on GPUs without any Xid. The miner now:

1. tells the failing GPU from the driver's Xid events (NVML): `faults: GPU3 (bus 04, RTX 3070) FAILED: Xid 13 ...; the workers of
   GPU0, GPU1, ... stopped with it: ... they are not at fault`;
2. remembers it in `gpu-faults.json` next to `uv.json` (crashes in a row, Xid, the offset it had);
3. restarts itself (Linux / HiveOS: in the same process, HiveOS does not count a restart; at most 12 times an hour, then it exits
   as before; Windows: start.ps1 restarts it);
4. with `--auto-uv`: a crashed undervolt offset counts as failed and the GPU is tuned below it; a crash at your own / kept offset
   **2 starts in a row** lowers it (`CRASHED while mining at +325 2 starts in a row ...: running it at +275`): the HiveOS profile
   Core Clock value first, then 60 MHz steps down to 0; a new offset of yours forgets the lowered one;
5. a GPU that still crashes with nothing left to lower (without `--auto-uv`: after 2 crash starts in a row) is **left out of mining
   for an hour**, the other GPUs mine. 30 min without a fault clear the count; `--uv-reset` does not clear `gpu-faults.json`
   (delete the file to clear it now).

---

## Русский

### 1. Что это

FreeForGem — GPU-майнер **Pearl (PRL)** (pearlhash, proof-of-useful-work) для видеокарт NVIDIA с **комиссией разработчика 1%**.
С 1.6.0 майнит также Common Foundry (CMFD, раздел 13), с 1.7.0 — **NOID** (Parano1d, раздел 15).

**Название.** С 1.7.0 мульти-монетный майнер (Pearl + CMFD + NOID) называется **FreeForGem** (бинарь `freeforgem`, кастомный
майнер HiveOS `freeforgem`, репозиторий `pepsykolya/FreeForGem`). **FreeForGemPear** остаётся майнером Pearl под своим именем.
Переход с FreeForGemPear: в HiveOS создайте **новый полётный лист** (Miner name `freeforgem` + новая Installation URL);
сохранённые результаты андервольта (`uv.json` из `/hive-config/freeforgempear` или из папки `freeforgempear` рядом с новой)
импортируются автоматически при первом запуске (в логе: `imported the saved undervolt results of FreeForGemPear`).
Собственное CUDA-ядро для Ampere: матрица A′ генерируется на кристалле, int8-GEMM на тензорных ядрах идёт в
персистентном ядре, а свёртка jackpot и хеш BLAKE3 слиты с ним. Главный цикл настроен вручную на уровне SASS.
Хост строит и проверяет доказательство каждой шары официальным верификатором `zk-pow` до отправки.

| Серия | Статус |
|---|---|
| RTX 30xx (Ampere, sm_86): 3060 / 3060 Ti / 3070 / 3070 Ti / 3080 / 3090 | **основная цель, оптимизировано: ~70 TH/s на RTX 3070 при ~107 Вт** |
| RTX 40xx (Ada, sm_89) | своё SASS-ядро с 1.5.4 (RTX 4070 Ti: тот же хэшрейт, на ~10 Вт меньше, чем с ядром RTX 30xx) |
| RTX 50xx (Blackwell, sm_120) | поддержка с 1.5.0, компилированное ядро (без оптимизации); нужен драйвер 570+ |
| CMP 50HX (TU102) | работает, но **для Pearl не рекомендуется: ~2.6 TH/s на карту** (int8 тензорные ядра урезаны ~в 17 раз относительно CMP 40HX, на любом драйвере) |
| RTX 20xx / GTX 16xx / CMP 30HX–50HX (Turing, sm_75) | поддержка с 1.5.3, собственное SASS-ядро: ~49,9 TH/s на RTX 2060 SUPER @1605 МГц; **память лочить на 5001, не 810** |
| AMD Radeon RX 6000 (RDNA2), Windows | **новое в 1.6.0**, своё ядро OpenCL: 34,4 TH/s на RX 6800 XT (раздел 14) |
| Старые карты, A100 (sm_80), AMD в Linux / HiveOS | не поддерживаются |

### 2. Требования

* Видеокарта NVIDIA: RTX 30 / RTX 40 (свои ядра), RTX 20 / GTX 16 / CMP, RTX 50 (с 1.5.0); драйвер NVIDIA **525 или новее** (RTX 50xx: **570+**).
* Или, **только в Windows**, карта **AMD Radeon RX 6000** (RDNA2) с драйвером AMD Adrenalin (с 1.6.0, раздел 14).
* HiveOS, Linux x86_64 (glibc 2.31+, Ubuntu 20.04+) или Windows 10/11 x64. CUDA toolkit, Python и WSL не нужны.
* Адрес кошелька PRL (`prl1...`) и интернет до stratum-пула Pearl.
* Видеопамять: ядро работает из L2-кэша и почти не использует VRAM.

### 3. Настройка в HiveOS (по шагам)

FreeForGem запускается в HiveOS как **Custom miner**: SSH и ручная установка не нужны.

**Ссылка для установки (1.4.9):**

```
https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.2-beta/freeforgem-1.7.2beta.tar.gz
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
   | Miner name | `freeforgem` (подставится сам после вставки ссылки) |
   | Installation URL | ссылка выше |
   | Hash algorithm | `pearlhash` (или оставить `----`) |
   | Wallet and worker template | `%WAL%.%WORKER_NAME%` |
   | Pool URL | `pearl-eu2.luckypool.io:3360` или Kryptex `prl-eu.kryptex.network:7048` (любой пул из раздела 6) |
   | Pass | пусто |
   | Extra config arguments | по желанию, например `--devices 0,1` |

4. **Применить.** *Apply changes*, *Create flight sheet*, затем применить лист к ригу.

Лог майнера: `/var/log/miner/freeforgem/freeforgem.log` и консоль майнера в HiveOS.
На панели видны хэшрейт по картам и общий, температура, вентиляторы, принятые и отклонённые шары.
Для обновления смените Installation URL на новую версию. То же подробнее: [HIVEOS.md](HIVEOS.md).

### 4. Linux без HiveOS

```
tar xzf freeforgem-1.7.2-beta-linux-x86_64.tar.gz
cd freeforgem-1.7.2-beta
./freeforgem --self-test        # по желанию: сверка GPU-ядра с CPU-эталоном
./freeforgem --pool pearl-eu2.luckypool.io:3360 --wallet ВАШ_АДРЕС_PRL --worker rig1     # LuckyPool
./freeforgem --pool prl-eu.kryptex.network:7048 --wallet ВАШ_АДРЕС_PRL --worker rig1       # Kryptex
```

| Опция | Значение |
|---|---|
| `--pool HOST:PORT` | stratum-пул (по умолчанию `pearl-eu2.luckypool.io:3360`); префикс `stratum+tcp://` допускается, SSL не поддерживается (нужен обычный TCP-порт) |
| `--wallet ADDRESS` | ваш адрес PRL (обязательно) |
| `--worker NAME` | имя воркера (по умолчанию `worker1`) |
| `--devices 0,1,...` | какие GPU использовать (по умолчанию все видимые; учитывает `CUDA_VISIBLE_DEVICES`) |
| `--minutes N` | остановиться через N минут (по умолчанию работает до Ctrl-C / SIGTERM) |
| `--self-test` | проверить ядро и выйти |

Один процесс ведёт все GPU и сам переподключается к пулу. Для постоянной работы запускайте под `systemd`, `screen` или `tmux`.

### 5. Windows 10/11

Нативная сборка: WSL, установка CUDA и Python не нужны. Нужен только драйвер NVIDIA.

1. Скачайте **`freeforgem-1.7.2-beta-windows-x64.zip`** со страницы релиза:
   `https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.2-beta/freeforgem-1.7.2-beta-windows-x64.zip`
   и сверьте SHA-256 (раздел 9).
2. Распакуйте в папку **без пробелов и русских букв**, например `C:\FreeForGem`.
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
* Без `start.bat`: `freeforgem.exe --pool pearl-eu2.luckypool.io:3360 --wallet ВАШ_АДРЕС_PRL --worker rig1`
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
cd /d C:\Mining\freeforgem-1.7.2-beta-windows-x64
:loop
powershell -NoProfile -Command "& { .\freeforgem.exe --pool pearl-eu2.luckypool.io:3360 --wallet ВАШ_АДРЕС_PRL --worker rig1 --auto-uv --uv-min 135 --uv-max 360 | Tee-Object -Append -FilePath C:\Mining\ffgp.log }"
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

**Замер FreeForGem, RTX 3070:** ядро `1560` МГц (лок), память `810` МГц, лимит мощности `120` Вт →
**~70 TH/s при ~105–107 Вт**; `--auto-uv` затем находит +195..240 (≈ 86–107 Вт).

**Стартовые значения по картам.** Строки *опорные* — опубликованные результаты настройки этих карт; строки *оценка* выведены
по картам того же поколения и не замерены. Все карты разные: свой offset пусть подберёт `--auto-uv`.

| Карта | Лок ядра, МГц | Память | V/F offset, МГц | Источник |
|---|---|---|---|---|
| RTX 3060 Ti, RTX 3070, RTX 3070 Laptop, RTX 3080 Ti | 1560–1650 | лок `810` | +200..255 | опорные (3070 — наш замер) |
| RTX 3060, RTX 3070 Ti, RTX 3080, RTX 3090 | 1560–1650 | лок `810` | +200..255 | оценка |
| CMP 70HX, CMP 90HX (Ampere) | 1650 | offset `-2000` (`--moffset`) | +255..300 | опорные, с FreeForGem не проверялись |
| RTX 4070 Ti, RTX 4090 | 2445 | лок `810` | +315..345 | опорные |
| RTX 4060, 4060 Ti, 4070, 4070 Super, 4070 Ti Super, 4080, 4080 Super | 1900–2445 | лок `810` | +300..345 | оценка |
| RTX 5070 Ti | 2445 | лок `810` | +480 | опорные |
| RTX 5070 | 2400 | лок `810` | проверки проходят до +390, выше — падение | отчёт пользователя, FreeForGemPear 1.5.1, Windows, драйвер 617.14 |
| RTX 5060, 5060 Ti, 5080, 5090 | 1900–2500 | лок `810` | +390..480 | оценка |
| RTX 2060 SUPER, RTX 2070/2080 (вкл. SUPER), GTX 1660/1650 | 1500–1650 | лок `5001` (**не 810**: на 810 Turing теряет ~25%) | +200..260; предел auto-uv +225 по умолчанию (с 1.6.2) | опорное (2060 SUPER: замер) |
| CMP 40HX | 1400 | лок `5001` | +150; предел auto-uv +150 по умолчанию (с 1.6.2) | риги пользователей; CMP 40HX ловила Xid на +255 |
| CMP 50HX (TU102) | — | лок `5001` | +220 и выше; предел auto-uv +225 по умолчанию (`--uv-max 225`) | риги пользователей. **Для Pearl не рекомендуется: ~2.6 TH/s на карту** (int8 тензорные ядра урезаны ~в 17 раз относительно 40HX; от драйвера не зависит: 550.78 и 595.91.07 одинаково, риг 3× CMP 50HX) |

RTX 40 и 50 работают на гораздо более высоком локе ядра (**1900–2500 МГц**) и держат больший offset, чем RTX 30; поэтому
auto-uv начинает поиск на RTX 40 с +200, на RTX 50 — с +300 (RTX 30 — с +120).

**Смешанный риг:** достаточно `--auto-uv` — пределы выбираются по каждой карте автоматически (с 1.6.2): RTX 20xx / GTX 16xx
до +225, CMP 30HX/40HX до +150 (CMP 50HX: +225), остальные до +495; при старте в логе виден диапазон каждой карты
(`auto-uv: GPU5 (bus 0b, CMP 40HX): range +120..+150 (max: CMP default; start: default)`). Для ручного управления — по значению
на карту в порядке `--devices`: `--uv-max 225,495,150` (`-` = значение по умолчанию для этой карты). Turing (RTX 20 / GTX 16 / CMP 30HX–50HX, sm_75) поддерживается с 1.5.3: в отличие от RTX 30 ему нужна память на 5001 (см. таблицу); CMP 170HX / A100 (sm_80) не поддерживаются.

**Карты CMP (CMP 30HX / 40HX / 50HX).** CMP 30HX/40HX майнят Pearl на Turing-ядре (память лочить на 5001, предел auto-uv +150).
**CMP 50HX для Pearl не рекомендуется** — всего ~2,6 TH/s на карту (int8 тензорные ядра урезаны ~в 17 раз относительно CMP 40HX,
на любом драйвере). **Для Common Foundry (CMFD) CMP 50HX подходит:** ~13 FW/s на карту при ~220 Вт (лимит мощности 225 Вт,
лок ядра 1410 МГц, память без лока; риг 4× CMP 50HX, пул Aria), с `--auto-uv` (подбор под CMFD, +225) — около 13,2 FW/s при
196–203 Вт (+8–11 % FW/kWh); см. раздел 13. CMFD нужно ~7 ГБ свободной видеопамяти, на картах с 6 ГБ (CMP 30HX) он не
запустится. **Разблокировка CMP не входит в FreeForGem:** драйверов и программ, снимающих ограничения CMP-карт, мы не
предоставляем и не поддерживаем; все цифры здесь — для карт как есть.

* HiveOS: вкладка Overclock → частота ядра (абсолютная), частота памяти (абсолютная), лимит мощности; V/F offset — через `--auto-uv`.
* Linux: `nvidia-smi -lgc 1560,1560`, `nvidia-smi -lmc 810,810`, `nvidia-smi -pl 120` (root) или `--lock-cclock` / `--lock-mclock`.
* Windows: `LOCK_CCLOCK` / `LOCK_MCLOCK` / `MOFFSET` в `start.bat` (от администратора), `start-nvidia-smi.bat` — если нужен
  лимит мощности, или MSI Afterburner. Готовые комбинации по картам — раздел 12, *Windows: готовые комбинации*.

Если пул отклоняет шары или в логе есть `bad`-попадания, снизьте частоту ядра или offset.

### 8. Как читать лог

| Строка | Значение |
|---|---|
| `FreeForGem 1.7.2-beta \| pool ... \| worker ... \| N GPU(s)` | стартовая строка |
| `[pearlgpu] search kernel: SASS-patched cubin (...)` | загружено настроенное ядро. `embedded search kernel could not be loaded (причина)` = майнер останавливается с ошибкой (драйвер старше 525 или неподдерживаемая карта) |
| `new job ... prepared in ...` | готово новое задание пула |
| `[ 120s] GPU0 ... \| total X TH/s \| hits ... \| user sub S acc A rej R \| fee sub ... acc ... rej ...` | каждые 10 с: хэшрейт по картам и общий; отправленные / принятые / отклонённые шары ваши и комиссии; `[DEVFEE]` пока идёт слот комиссии С 1.5.3 у каждой карты также видны потребление и эффективность, например `GPU0 49.906 (151 W, 0.330 TH/W)`, и итог `\| 151 W, 0.330 TH/W` (если доступен NVML) |
| `[DISCONNECTED]` | соединение с пулом потеряно; майнер переподключится сам |
| `devfee: ...` | слоты комиссии (раздел 10) |
| `!! ...` | предупреждение или ошибка (остановился GPU-воркер, hit не отправлен) |

### 9. Проверка загрузки (SHA-256)

Сравните результат с хешем на странице релиза.

* Linux: `sha256sum freeforgem-1.7.2-beta-linux-x86_64.tar.gz freeforgem-1.7.2beta.tar.gz`
* Windows: `certutil -hashfile freeforgem-1.7.2-beta-windows-x64.zip SHA256` или
  `Get-FileHash freeforgem-1.7.2-beta-windows-x64.zip -Algorithm SHA256`

### 10. Комиссия — 1%

Один 60-секундный слот в каждом 100-минутном цикле, в случайный момент цикла, на **том же пуле, который вы указали**, под отдельным
логином (воркер `ffgp-devfee`). В логе: `devfee: next slot in N min`, `devfee: mining 60s for FreeForGem dev (1%)`,
`devfee: back to user`. Первый слот приходится на случайный момент первых 100 минут. Если логин комиссии не обслуживается, слот
пропускается; ваш майнинг не блокируется. На LuckyPool логин комиссии с 1.4.2 майнит заодно и NOCK разработчика
(`prl1...+NOCK`); если пул не примет такой логин, майнер вернётся к обычному кошельку PRL. Время комиссии остаётся 1%. Комиссия — условие лицензии: убирать, уменьшать или перенаправлять её нельзя
(см. [LICENSE](LICENSE), пункт 3).

### 11. Проблемы и решения

| Симптом | Решение |
|---|---|
| Проверить, что всё работает | `freeforgem --self-test` (Windows: `freeforgem.exe --self-test`) должен закончиться строкой `== self-test PASSED ==` |
| При старте `embedded search kernel could not be loaded` | обновите драйвер NVIDIA (525+; **для RTX 50xx нужен 570+**) и проверьте, что карта Ampere (RTX 30xx) или новее |
| Высокая загрузка CPU | по умолчанию ожидание GPU спит (заметно меньше 10% ядра на карту). `PEARL_SPIN=1` включает активное ожидание (ядро CPU на карту): убедитесь, что она не задана |
| Ошибки `pg_init(...)` или нехватка памяти | видеокарту занимает другая программа (игры, локальные нейросети). Закройте её или выберите карты через `--devices` |
| Долго `[DISCONNECTED]` | проверьте хост/порт пула и брандмауэр (исходящий TCP); попробуйте другой регион из раздела 6 |
| Много отклонённых шаров | снизьте частоту ядра (раздел 7), проверьте охлаждение |
| Хэшрейт падает через несколько минут, в Диспетчере задач нагрузка *Video Encode* | демонстрация экрана, запись или стрим (демонстрация экрана в Discord, OBS, NVIDIA App / Instant Replay) работают на той же видеокарте и отнимают её долю; остановите их или выключите в этой программе аппаратное ускорение |
| HiveOS пишет `Already installed` и ничего не меняется | Installation URL должен указывать на новую версию |
| Windows: окно сразу закрывается | запустите `start.bat` из `cmd`, чтобы увидеть сообщение; в пути не должно быть пробелов и русских букв |
| Windows: `.exe` блокирует Defender / SmartScreen | *Подробнее → Выполнить в любом случае* или добавьте папку в исключения; сверьте SHA-256 |

Переменные окружения (задаются до запуска майнера): `PEARL_SPIN=1` (активное ожидание вместо сна).
`FFGP_M=N` задаёт число строк A за попытку (кратно 128). По умолчанию `m` равен 47104, и майнер сам подбирает `m` для каждой GPU по числу её SM (`m = 128 * SM * r`), поэтому на риге с разным числом SM каждая карта работает со своим `m` — по одному заданию на группу SM (с 1.4.7). Задавайте `FFGP_M` только для экспериментов: он задаёт один `m` на всех устройствах, а чем больше `m`, тем немного выше производительность, но тем больше расход оперативной памяти хоста.
В Windows: `set PEARL_SPIN=1` в окне `cmd`, затем запустите `start.bat` из этого же окна.

### 12. Андервольт / `--auto-uv` (необязательно, новое в 1.4.0)

**Видеоуроки.** Пошаговые видео по настройке андервольта приложены к релизу на GitHub отдельными файлами (страница Releases → нужный релиз → *Assets*): на русском `FreeForGemPear-1.5.2-undervolt-RU.mp4` и на английском `FreeForGemPear-1.5.3-undervolt-EN.mp4`.

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
   Выше +495 не поднимается (`--uv-max`, максимум +600; до 1.5.1 было +300 с жёстким потолком +400); с 1.6.2 на Turing
   (RTX 20xx / GTX 16xx) — выше +225, на CMP 30HX/40HX — выше +150 (CMP 50HX — выше +225), если `--uv-max` не задан. Карты RTX 40/50 часто держат +450..480.
   Сохранённый результат выше уменьшенного предела не применяется как есть: карта работает на пределе (после быстрой проверки),
   а сохранённый результат остаётся на случай, когда предел снова поднимут; если результат *равен* старому пределу, при
   повышении предела карта подбирается заново.
4. Результат сохраняется в `uv.json` с ключом: UUID карты, версия драйвера, сборка ядра. При следующих запусках он
   применяется после быстрой проверки (около 30 с). Если изменились драйвер, сборка ядра или ваш начальный offset,
   карта подбирается заново.
   Каждая проверка идёт в отдельном процессе, которому нужно около 2,4 ГБ ОЗУ (`FFGP_UV_CHECK_MB`): на риге с малым объёмом
   ОЗУ проверки нескольких карт идут одновременно, только пока хватает свободной памяти, остальные ждут (`waits for RAM` в
   логе) и идут следом; майнинг начинается, когда все карты подобраны или применены. Во время майнинга перепроверка сначала
   получает память и только потом ставит карту на паузу (с 1.7.1) — карта, ждущая ОЗУ, продолжает майнить.
5. Во время майнинга: каждые 6 часов (`--uv-reval-hours`) настройка перепроверяется (майнинг на этой карте приостанавливается
   примерно на 30 с). Если найденный результат не прошёл проверку на CPU или пошла серия отклонённых шар, offset
   снижается на шаг (но не ниже вашего значения) и сохраняется. **Применение разгона (с 1.7.1):** `nvidia-oc` в HiveOS при
   каждом старте майнера и каждой смене OC записывает значение *Core Clock* из профиля разгона, **0, если поле пустое** — до
   1.7.1 это выключало андервольт до следующего перезапуска (риг 4× RTX 3070 ел 604 Вт вместо ~440). Теперь offset, который
   прочитан как 0, как значение профиля HiveOS или как ваш исходный offset, считается применением разгона: auto-uv ставит
   проверенный offset обратно после быстрой проверки (майнинг на карте встаёт на ~30 с; при ошибке — шаг вниз, как при
   перепроверке; не чаще 4 раз в час), в логе `the offset was reset to +0 by an OC apply (...); re-applying the verified +285
   after a quick check`. Проверка, во время которой разгон сбросил offset, повторяется. Любое другое значение — ваш собственный
   выбор: auto-uv отступает на этой карте и не воюет.
6. При выходе (Ctrl+C, SIGTERM, `miner stop` в HiveOS, закрытие окна консоли в Windows, обычное завершение) ваш offset
   возвращается на всех GPU (до 1.6.1 `miner stop` в HiveOS мог вернуть только GPU0). Если майнер был убит (`kill -9`, сбой), то при
   следующем запуске ваш offset сначала восстанавливается из файла `uv-restore.json`. Перезагрузка в любом случае сбрасывает
   offset в драйвере. Остановка во время проверки (`miner stop` / `miner restart` в HiveOS шлёт Ctrl+C, а потом закрывает
   screen-сессию) — не провал (с 1.7.1, также для CMFD и NOID): проверяемый offset не считается точкой сбоя и не ставит
   auto-uv на паузу 24 ч; следующий запуск продолжает как обычно.

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
     `set "UV_MAX="` (пусто = по карте: +495, RTX 20xx/GTX 16xx/CMP 50HX +225, CMP 30HX/40HX +150; RTX 40/50 часто держат +450..480, RTX 30
     обычно +200..360). Список через запятую задаёт значение для каждой карты в порядке `DEVICES`, например `set "UV_MAX=225,495,150"`.
   * `start-nvidia-smi.bat`: `AUTO_UV=1`, диапазон в `UV_MIN` / `UV_MAX`, плюс лимит мощности и фиксация частот через `nvidia-smi`.
   * своя командная строка: `freeforgem.exe --pool ... --wallet ... --auto-uv --uv-max 480`.
4. **Точка старта.** Подбор начинает с offset, который сейчас стоит на карте, и ниже него не опускается. Смещение *Core Clock*
   в MSI Afterburner (и похожих программах) — это тот же самый offset: выставленное там значение станет точкой старта и
   вернётся при выходе. Если Afterburner заново применит профиль во время майнинга (offset вернётся к вашему значению или к 0),
   auto-uv после быстрой проверки поставит проверенный offset обратно (с 1.7.1); любое другое значение — и он отступит на этой
   карте. Если offset не задан, подбор стартует с +120, на RTX 40 — с +200, на RTX 50 — с +300 (или с `--uv-min` / `UV_MIN`).
5. **Подождите.** При первом запуске подбор идёт около 15 минут на карту (все карты параллельно), майнинг начинается после
   него. Следующие запуски применяют сохранённое значение после проверки ~30 с.
6. **Файлы** лежат в папке майнера: `uv.json` (результаты) и `uv-restore.json` (ваш исходный offset). Папка должна быть
   доступна на запись, поэтому не кладите майнер в `C:\Program Files`. Подобрать заново: закройте майнер, удалите `uv.json`
   (или запустите в этой папке `freeforgem.exe --uv-reset`) и запустите снова. Новый драйвер или новое ядро майнера
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
   * `start offset ... leaves no room below the cap` — ваш offset уже равен `UV_MAX` (или пределу карты по умолчанию, он
     указан в скобках) или выше; поднимите `UV_MAX`, если уверены, что карта держит больше.
   * `no safe offset above ... was found` — карта не держит больше вашего значения; ничего не меняется.
8. **Если ПК завис или сбросился драйвер** (чёрный экран, «видеодрайвер перестал отвечать»): перезагрузитесь и запустите
   майнер снова — он сначала вернёт ваш offset из `uv-restore.json`. Если сам майнер упал на шаге подбора (в окне
   `Miner exited (code -1073741819)`), следующий запуск считает этот шаг провальным и подбирает только ниже него (с 1.5.2;
   раньше подбор начинался заново и падал на том же шаге). С 1.5.3 он ещё и не проходит заново уже пройденные шаги:
   после прогрева сразу подтверждение (около 5 минут вместо полного подбора). Если карта сбоит снова и снова, уменьшите `UV_MAX` или
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
  `set "LOCK_CCLOCK=1560,2445"`. Для андервольта достаточно `AUTO_UV=1`: пределы выбираются по каждой карте (RTX 20xx/GTX 16xx/CMP 50HX
  +225, CMP 30HX/40HX +150, остальные +495; RTX 40/50 сами стартуют с +200/+300). Для ручного управления `UV_MIN` / `UV_MAX` / `UV_OFFSET`
  принимают список в том же порядке, например `set "UV_MAX=225,495,150"` (`-` = значение по умолчанию для карты). В списке должно
  быть ровно по одному значению на карту.
* **Быстрее подбор:** `set "UV_STEP=30"` вдвое сокращает число шагов (результат грубее). На RTX 50 больше всего времени
  экономит высокий старт (около +300): карта его всё равно держит, и поиск начинается сразу оттуда (с 1.6.2 это значение по
  умолчанию на RTX 50). Не завышайте: если
  карта не проходит проверку уже на `UV_MIN`, подбора не будет и останется ваш offset (это видно в логе; тогда уменьшите `UV_MIN`).
* **Offset уже известен:** `set "UV_OFFSET=480"` (а `AUTO_UV` не включать) — применится после одной быстрой проверки, без поиска.
* **Нужен ещё лимит мощности:** используйте `start-nvidia-smi.bat` (`CCLOCK` / `MCLOCK` / `PLIMIT`, `UV_MIN` / `UV_MAX` / `UV_STEP`).
* **Подобрать заново после смены диапазона:** на один запуск `set "EXTRA=--uv-reset"`, потом снова очистите.

**Риск.** При слишком агрессивном андервольте возможен сброс драйвера или зависание (подбор идёт только по тем значениям, которые
только что проверены, и с запасом, но кристаллы, температура и драйверы разные). После зависания перезагрузитесь: при следующем
запуске ваш offset будет восстановлен первым делом. Используйте на свой риск, как и любой разгон.

**Включение**
* HiveOS: добавьте `--auto-uv` в *Extra config arguments* полётного листа (подробнее в HIVEOS.md).
* Linux: `sudo ./freeforgem --pool ... --wallet ... --auto-uv`
* Windows: правой кнопкой по `start.bat` → *Изменить*, уберите `rem` перед `set "AUTO_UV=1"`; запускайте `start.bat` **от администратора**.

| Параметр | Назначение |
|---|---|
| `--auto-uv` | подобрать, применить и сопровождать андервольт (как описано выше) |
| `--uv-offset N` | применить фиксированный offset N МГц после быстрой проверки, без подбора (если проверка не прошла, остаётся ваш offset) |
| `--uv-reset` | забыть сохранённые результаты; вместе с `--auto-uv` карты подбираются заново; без кошелька майнер просто завершается |
| `--uv-max N` | наибольший offset, который пробует подбор (максимум 600; по умолчанию по карте: RTX 20xx / GTX 16xx / CMP 50HX 225, CMP 30HX/40HX 150, остальные 495; в Windows: `set "UV_MAX=480"` в `start.bat`) |
| `--uv-min N` | offset, с которого начинается подбор и ниже которого не опускается (по умолчанию offset из HiveOS/драйвера, иначе 120; RTX 40 — не ниже 200, RTX 50 — не ниже 300) |
| `--uv-step N` | шаг подбора в МГц (по умолчанию 15) |
| `--uv-keep-last` | после неудачного шага оставить последний удачный шаг (по умолчанию — последний удачный минус один шаг) |
| `--uv-reval-hours H` | интервал перепроверки во время майнинга (по умолчанию 6, `0` — не перепроверять) |
| `--uv-dir DIR` | где лежат `uv.json` и `uv-restore.json` (также `FFGP_STATE_DIR`) |
| `--auto-uv-dry-run` | только пишет в лог, какие карты были бы изменены (устройство CUDA → устройство NVML, шина); ничего не меняет |

`--uv-max`, `--uv-min` и `--uv-offset` принимают одно значение для всех карт или список через запятую — ровно по одному
значению на карту в порядке `--devices` (без `--devices`: GPU0, GPU1, ... как в логе; в HiveOS это порядок PCI-шины, как в
панели). `-` в списке = значение по умолчанию для этой карты (`--uv-offset`: карта не трогается). Список неверной длины —
ошибка, он не дополняется. Явный `--uv-max` применяется как задан, даже выше предела карты по умолчанию; тогда в логе
предупреждение (`WARNING --uv-max +495 is above the recommended +150 for this card (CMP default)`). Пример для 2× RTX 2060 SUPER,
3× RTX 3070 Laptop и CMP 40HX: `--auto-uv --uv-max 225,225,495,495,495,150` (то же, что по умолчанию).

**Управление частотами GPU (по желанию, через NVML; нужен root / администратор, работает в Windows — с 1.4.9).** `--lock-cclock N` фиксирует частоту ядра на N МГц (как `nvidia-smi -lgc N,N`), `--lock-mclock N` фиксирует частоту памяти на N МГц (как `-lmc`), `--moffset N` задаёт смещение частоты памяти в МГц, может быть отрицательным (нужен драйвер 555+). Каждая опция принимает одно значение для всех GPU или список через запятую в порядке `--devices` (по одному на карту). Настройки применяются при старте и снимаются при выходе (Ctrl-C / SIGTERM / сбой). **С 1.7.1 прежний лок возвращается, а не снимается:** в HiveOS — лок из профиля разгона (*Lock Core Clock* / *Lock Memory Clock*, читается из `/hive-config/nvidia-oc.conf`; до 1.7.1 тестовый запуск с `--lock-cclock` оставлял риг без лока HiveOS); вне HiveOS NVML не умеет читать лок, поэтому укажите его в `FFG_RESTORE_CCLOCK` / `FFG_RESTORE_MCLOCK` (имена FreeForGemPear `FFGP_RESTORE_*` тоже работают; одно значение или список по картам, 0 = лока не было), иначе частота, как и раньше, сбрасывается по умолчанию. В логе видно, что происходит: `core clock locked to 1500 MHz; lock before: 1470 MHz (HiveOS OC profile), put back on exit`, при выходе `core clock lock 1470 MHz put back`. После сбоя или `kill -9` то же делает следующий запуск. В Windows задайте `LOCK_CCLOCK`, `LOCK_MCLOCK`, `MOFFSET` в `.bat`-файле. Пример: `--lock-cclock 1500 --lock-mclock 810 --moffset -200`.

Файлы по умолчанию лежат рядом с программой (Windows — в папке майнера), в HiveOS — `/hive-config/freeforgem`
(переживает обновления майнера и перезагрузки). **Сброс:** `freeforgem --uv-reset` (или удалить `uv.json`).

**Строки лога** (все начинаются с `auto-uv:`, так что `grep auto-uv` показывает всю историю):
```
auto-uv: GPU0 (bus 0d, RTX 3070): CUDA device 0 -> NVML index 1, PCI 0000:0D:00.0, GPU-0d06..., current offset +150
auto-uv: GPU0 (bus 0d, RTX 3070): range +150..+495 (max: default; start: current offset)
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
   `dmesg` / сообщениях HiveOS — `Xid` (13, 31, 43, 79 ...). `freeforgem --uv-offset N` применяет N, выполняет ту же
   проверку, что и `--auto-uv`, и майнит с ним — это быстрый способ проверить кандидата.
4. Повторяйте с шага 2 до первой проблемы, затем вернитесь на **два шага (30 МГц)** от значения, где она появилась, и оставьте его.
5. Проверьте заново после обновления драйвера, нового кулера или в жаркую погоду.

### 13. Common Foundry (CMFD), новое в 1.6.0

Тот же бинарник майнит и **Common Foundry (CMFD, ForgeMatrix V4)**. Работу по CMFD делает движок **FreeForgeMiner**
(MIT, <https://github.com/pepsykolya/freeforgeminer>, релиз 1.2.4), он лежит в папке `cmfd/` пакета: в Windows —
релизные файлы без изменений, в Linux/HiveOS — собранные из того же закреплённого исходника на Ubuntu 20.04, чтобы работать на любом
HiveOS (образы на Ubuntu 20.04 и 22.04).
FreeForGem готовит модель, запускает по процессу движка на каждую GPU, применяет ваши настройки частот, печатает свою
обычную строку статистики и перезапускает упавший движок.

**Монета определяется по адресу пула**: адрес `cmfd+tls://` — это CMFD (`--algo cmfd|pearl` задаёт явно). Кошелёк —
ваш адрес CMFD в любой форме: `cmfd1...` (63 символа с контрольной суммой, так его показывает кошелёк
Common Foundry 1.0.16+) или прежние 64 шестнадцатеричных символа — это один и тот же ключ. Адрес `cmfd1...` проверяется
(опечатка отсекается до начала майнинга) и уходит на пул в hex. Адреса тестовой сети `tcmfd1...` не принимаются.

**Требования.** NVIDIA RTX 20xx и новее (оптимизировано под RTX 30/40), около **7 ГБ свободной видеопамяти на карту**
(одна модель — 6,4 ГБ), драйвер NVIDIA R575+ на Linux/HiveOS, 7 ГБ на диске под модель. Linux/HiveOS: glibc 2.30+ (Ubuntu 20.04 и новее, любой образ
HiveOS).

**Модель** (6,4 ГБ) скачивается один раз с официальных серверов Common Foundry проверяемыми частями, дальше при каждом
запуске только проверяется (10–40 с). Где лежит: `--cmfd-model DIR`; по умолчанию `/hive/miners/custom/cmfd-model` в HiveOS
(общая с FreeForgeMiner: риг, где он уже работал, повторно не качает), иначе `cmfd-model` рядом с бинарником.

**Пулы** (адрес вместе с пином сертификата, ровно как ниже; в скобках комиссия пула):

| Пул | Адрес |
|---|---|
| **Aria / AriaBrain** (по умолчанию, pool.ariabrain.com/cmfd.html, 3 %) | `cmfd+tls://159.69.194.46:29445?pin=9dfb51083f287726117f689f87bc7a878792efcca58ca8b6f6e7c05ac5d152e9` |
| **Midas Mining** (midasmining.net, 1 %) | `cmfd+tls://24.107.152.151:29745?pin=2ee4e6a028b07b300753dff3c260b79763c5e748d4e78b86cf3855e97eb2e80e` |
| cmfd-pool.online (3 %) | `cmfd+tls://109.199.124.187:29465?pin=ebe88f5e05f3a222208d551d05b6d39057b64ce8239ba7a708d487e15ac711be` |

Адрес и пин публикует сам пул (Midas: `https://midasmining.net/api/v1/pool`, поле `public_pool_url`). Сменился пин — значит
сменился сертификат: возьмите новую строку у пула, пин не отключайте. Некоторые пулы (cmfd-pool.online) примерно на час
банят **IP-адрес** после потока отклонённых шар или переподключений — не проверяйте на них нестабильный разгон.

**Linux:**
```
./freeforgem --pool 'cmfd+tls://159.69.194.46:29445?pin=9dfb51083f287726117f689f87bc7a878792efcca58ca8b6f6e7c05ac5d152e9' \
    --wallet cmfd1ВАШ_АДРЕС --worker rig1
# Midas Mining:
./freeforgem --pool 'cmfd+tls://24.107.152.151:29745?pin=2ee4e6a028b07b300753dff3c260b79763c5e748d4e78b86cf3855e97eb2e80e' \
    --wallet cmfd1ВАШ_АДРЕС --worker rig1
./freeforgem --algo cmfd --self-test        # проверка GPU без пула: точное сравнение с эталонным путём; при наличии модели
                                                # ещё официальный тест-вектор Common Foundry 1.0.16 (32 nonce, SHA-256)
```
В оболочке адрес берите в кавычки (`?` — шаблон). Запускайте от root, если майнер должен сбросить андервольт-смещение (ниже).

**HiveOS** (полётный лист, Miner *Custom*, тот же пакет, что и для Pearl): *Hash algorithm* `forgematrix_v4`, *Wallet and worker
template* `%WAL%.%WORKER_NAME%`, кошелёк — ваш адрес CMFD, *Pool URL* — строка из таблицы, например
`cmfd+tls://159.69.194.46:29445?pin=9dfb51083f287726117f689f87bc7a878792efcca58ca8b6f6e7c05ac5d152e9`
(Midas: `cmfd+tls://24.107.152.151:29745?pin=2ee4e6a028b07b300753dff3c260b79763c5e748d4e78b86cf3855e97eb2e80e`).
*Extra config* по желанию: `--cmfd-mode eco`, `--devices 0,1`, `--per-gpu-workers`, `--lock-cclock 1560`.
HiveOS показывает FW/s по каждой карте; первый запуск на риге качает модель (смотрите лог майнера).
Дайте полётному листу CMFD **отдельный профиль разгона без лока памяти** (см. ниже *Частота памяти*): разгон под Pearl
режет CMFD примерно в 6 раз.

**Windows:** `start-cmfd.bat` — впишите `WALLET`, оставьте `POOL` (Aria) или уберите `rem` у строки Midas (или другой),
по желанию `DEVICES`, `MODE`, локи частот. Если на карте стоит андервольт-смещение — запускайте от имени администратора.

**Параметры:** `--cmfd-mode speed|eco` (speed по умолчанию; eco: RTX 30 примерно −13 % мощности за −2 % хешрейта, у RTX 40/50
одинаково), `--cmfd-batch N` (1–64, по умолчанию по памяти: 64 на 11 ГБ+, иначе 32), `--cmfd-model DIR`, `--cmfd-dir DIR`
(папка движка), `--per-gpu-workers` (`WORKER.gpuN` на пуле вместо одного воркера на риг), `--lock-cclock/--lock-mclock/
--moffset` как в разделе 7, `--devices`, `--minutes`.

**Частоты и андервольт.** CMFD нагружает тензорные ядра гораздо сильнее Pearl. Смещение V/F ядра, нормальное для Pearl
(+200 на RTX 3070, +330 на RTX 4070 Ti), ломает CMFD сразу («illegal memory access», «misaligned address») или даёт
неверные результаты. Поэтому в режиме CMFD майнер **сам сбрасывает положительное смещение ядра в 0** перед стартом движка и
возвращает прежнее значение при выходе (в логе `cmfd offset: GPU0: core offset +345 -> 0 MHz ...`; после аварийного
завершения следующий запуск сначала возвращает его). Нужны права root/администратора; без них в логе будет предупреждение, и
смещение надо убрать самому (HiveOS — профиль разгона полётного листа CMFD). `--uv-offset N` ставит N — **на ваш риск**.
На **AMD Radeon** `--auto-uv` подбирает андервольт для CMFD через amduv (раздел 14, с 1.7.1); на NVIDIA — ниже.
Безопасно: стоковые частоты, лимит мощности или лок ядра (`--lock-cclock`).

**`--auto-uv` для CMFD (NVIDIA).** С `--auto-uv` (HiveOS — *Extra config arguments*; Windows — `AUTO_UV=1` в
`start-cmfd.bat`, запуск от администратора) майнер подбирает андервольт **именно под CMFD**, отдельно от результата Pearl.
После подготовки модели и до старта движка, на каждой карте: родной GPU-воркер движка (`cmfd-v4-replay`, без изменений)
один раз грузит модель и гоняет две фиксированные пачки; на базовом смещении (0, сток) их финальные дайджесты берутся
эталоном и прогоняется официальный тест-вектор Common Foundry 1.0.16; затем смещение ядра растёт шагами `--uv-step` (15 МГц)
от `--uv-min` (по умолчанию — шаг над 0) до `--uv-max` (по умолчанию — лимит карты из раздела 12: 495, RTX 20xx / CMP 50HX
225, CMP 30HX/40HX 150); каждый шаг — 30 с + 10 с замера, и **каждый дайджест должен совпасть побитно** с эталоном (один
неверный бит где угодно в 384 слоях его меняет). Первый упавший шаг — край; результат — на шаг ниже (`--uv-keep-last`
оставляет последний удачный), подтверждение 2 минуты + тест-вектор (при ошибке ещё шаг вниз). Результат сохраняется в
`uv.json` (раздел `cmfd`, по UUID карты, с драйвером и идентичностью движка/воркера), следующие запуски применяют его сразу
(или после 30-секундной проверки, если он проверялся больше `--uv-reval-hours` назад). Во время майнинга `INTEGRITY | ERROR`
движка или CUDA-сбой на тюненой карте — шаг назад (с сохранением) и перезапуск движка. Если применение разгона сбросило
подобранное смещение во время майнинга (`nvidia-oc` в HiveOS при старте майнера и после смены OC пишет *Core Clock* профиля,
0 при пустом поле), оно возвращается (с 1.7.1): движок этой карты останавливается, сохранённое смещение проходит 30-секундную
быструю проверку на модели + тест-вектор (при ошибке — шаг назад), движок перезапускается на нём; не чаще 4 раз в час, любое
другое значение — тюнер отступает на этой карте. Проверка, во время которой разгон сбросил смещение, повторяется. Ваше смещение возвращается при
выходе (после аварии — при следующем запуске), как и без `--auto-uv`. Если проверка не может работать (нет модели, нет
воркера, ошибка на базовом смещении) — карта майнит на 0 с предупреждением WARNING. Подбор занимает около 2 + 0,75 × число
шагов минут на карту (все карты одновременно). Лог: `grep "auto-uv (cmfd)"`.
Замеры (Aria, среднее строки статистики): RTX 3070 с лимитом 135 Вт упирается в мощность на ~1050 МГц, где напряжение уже
на дне кривой: край +225, результат +195, выигрыша нет (13,84 → 13,97 FW/s при 135 Вт). Та же карта при 220 Вт: сток 19,5 FW/s
при 214 Вт (328 тыс. FW/kWh) → +135 (край +165) 20,4–20,5 FW/s при 213–216 Вт (342–345 тыс., **+4–5 %**). CMP 50HX (лимит
225 Вт, лок ядра 1410 МГц): края до своего предела +225 нет; 13,10 FW/s при 216 Вт → 13,21 при 196 Вт (**+11 %**), 13,01 при
217 Вт → 13,17 при 203 Вт (**+8 %**). **Край зависит от рабочей точки**: результат подбирается заново, если лимит мощности
изменился больше чем на 3 % или изменился `--lock-cclock`; после смены лока частоты ядра в профиле разгона (HiveOS,
nvidia-smi) запустите один раз с `--uv-reset`.

**Частота памяти: для CMFD без лока памяти.** Pearl память почти не нужна, CMFD — нужна. Обычный разгон под Pearl с низким
локом памяти (в HiveOS *Mem lock* 810, `nvidia-smi -lmc 810`, `--lock-mclock 810`) режет CMFD примерно **в 6 раз**: на риге
HiveOS с 4× RTX 3070 карта с памятью, залоченной на 810 МГц, дала **3,7 FW/s**, со свободной памятью — **21,8 FW/s** (~237 Вт
при лимите 240 Вт). Сделайте для CMFD **отдельный профиль разгона**: без лока памяти, смещение ядра 0, лимит мощности по
вкусу. С 1.6.1 майнер один раз на каждую GPU через 30 с майнинга проверяет это и предупреждает
(`[GPU0] WARNING: memory clock 810 MHz of 7001 MHz while mining CMFD ...`); сам ваш разгон он не меняет.

**Лог.** Строки движка идут с префиксом `[GPUn]` (`MINER STATS`, `INTEGRITY | ok`, `SHARE STALE` — безвредно, пока шара
летела, пришёл новый блок; `SHARE REJECTED` — неверный результат, снизьте частоты). Раз в 10 с FreeForGem печатает
`[ 290s] GPU0 37.04 (190 W, 701390 FW/kWh) | total 37.04 FW/s | ... | user sub 84 acc 80 rej 0 | stale 4 | height 25089`
(`[DEVFEE]` во время слота комиссии; `STARTING` — движок грузит модель; `RESTARTING`, `DISABLED`).
Упавший движок перезапускается через 15/30/60/120/300 с; после 5 неудачных запусков подряд (каждый короче 2 минут) эта GPU
оставляется в покое, причина — в логе (никаких бесконечных переподключений, за которые банят IP).

**Комиссия для CMFD: 1 %**, внутри движка: 36 секунд в час на GPU, в случайный момент, на выбранном вами пуле, воркер
`ffm-fee`, строки лога `DEV FEE | ...`. Комиссия Pearl из раздела 10 в режиме CMFD не работает (итого 1 %).

**Хешрейт** (пул Aria, прогоны 5–30 минут, смещение 0, батч авто): RTX 4070 Ti (лимит 190 Вт) 36,8 FW/s при 187 Вт;
RTX 3070 (лимит 135 Вт) 14,1 FW/s при 135 Вт — столько же, сколько FreeForgeMiner 1.2.3 на тех же картах (36,8 / 14,2 FW/s).
Со стоковым лимитом мощности RTX 3070 даёт около 20 FW/s (HiveOS, лимит 240 Вт, память без лока: 21,8 FW/s при ~237 Вт).

**Проблемы:** `out of memory` — закройте программы, занимающие GPU, CMFD нужно ~7 ГБ свободных; `illegal memory access` /
`misaligned address` / `INTEGRITY | ERROR` — уберите смещение ядра, снизьте частоты; `model preparation ... failed` — проверьте
доступ к downloads.commonfoundry.ai и github.com и место на диске; пул не принимает адрес — возьмите актуальную строку (пин) на
сайте пула.


### 14. AMD Radeon (Windows), новое в 1.6.0

Пакет для Windows майнит и на картах **AMD Radeon** — своим ядром OpenCL (`pearlgpu_amd.dll`, закрытое, как и CUDA-ядро).
Тот же бинарник, те же файлы запуска, тот же лог; библиотеку GPU майнер выбирает сам:

* `--backend auto` (по умолчанию): CUDA-библиотека, если она видит карту NVIDIA, иначе AMD. `--backend amd` / `--backend cuda`
  (или `FFGP_BACKEND`) задают выбор явно. В логе видно, что выбрано: `1 GPU(s) [amd]` и
  `[pearlgpu-amd] gfx1030: amd-a0 udot4 k8192`.
* Используются только **дискретные** карты Radeon; встроенная графика Ryzen «AMD Radeon(TM) Graphics» пропускается.
* Один процесс — один производитель: на ПК с картами NVIDIA и AMD запустите два майнера (`--backend cuda` и `--backend amd`).

| GPU AMD | Статус |
|---|---|
| RX 6000 (RDNA2, напр. RX 6800 XT) | **проверено**: Pearl 34,4 TH/s при ~260 Вт (сток), self-test, шары на пуле; CMFD ~10 FW/s |
| RX 7000 (RDNA3), RX 9000 (RDNA4) | должно работать на том же ядре, **не проверено** |
| RX 5000 (RDNA1), Vega и старше, встроенная графика | не поддерживаются |
| Linux / HiveOS с AMD | **пока нет** (пакеты для Linux и HiveOS — только NVIDIA) |

**Нужно:** Windows 10/11 x64 и драйвер AMD Adrenalin (в нём OpenCL); драйвер NVIDIA не нужен. Запуск — любой `start*.bat`,
как для NVIDIA. Проверка: `freeforgem.exe --self-test` (ядро AMD сверяется с эталоном на CPU, `== self-test PASSED ==`).

**Занятый CPU:** с AMD майнер ждёт GPU активно и работает с приоритетом «выше среднего», поэтому CPU-майнер или другая
нагрузка на все ядра не оставляет GPU без работы (33,7–33,9 вместо 32,6 TH/s рядом с CPU-майнером на всех ядрах). Один поток
CPU при этом занят; `PEARL_AMD_SPIN=0` / `FFGP_PROC_PRIO=0` отключают это.

**Ватты** в строке статистики — от драйвера (ADLX, мощность чипа, только чтение). На риге из нескольких карт AMD ватты
отдельных карт могут перепутаться между картами (сумма по ригу верна).

**Андервольт (`--auto-uv`) на AMD** устроен иначе, чем у NVIDIA: майнер запускает лежащую рядом утилиту `amduv.exe`, она один
раз на карту / драйвер / версию AMD-библиотеки (~10–15 мин) подбирает предел частоты ядра на минимальном напряжении (каждая точка
проверяется побитно на GPU и на CPU), а при следующих запусках применяет результат. Отдельный сторож **возвращает ваши
настройки частоты / напряжения / лимита мощности**, когда майнер завершается, в том числе после падения или kill. Запуск от
администратора. На нашей RX 6800 XT: около 30–32 TH/s при 160–172 Вт вместо 34 TH/s при ~260 Вт (+40 % TH/Вт).
`$FFGP_AMD_UV_GOAL=max` оставляет больше скорости; `--uv-reset` забывает результат. `--uv-offset`, `--lock-cclock`,
`--lock-mclock` и `--moffset` — только для NVIDIA (с AMD майнер их не принимает).

**CMFD на AMD:** `start-cmfd.bat` / `--pool cmfd+tls://…` работают так же; пакет запускает свой OpenCL-воркер
`cmfd\cmfd-v4-replay-ocl.exe` и клиент пула `cmfd\cmfd-miner-ffgp.exe` (FreeForgeMiner, пересобранный с небольшим патчем из
`cmfd/patches/`, чтобы работать без nvidia-smi). Нужно ~7 ГБ свободной видеопамяти. RX 6800 XT: ~10 FW/s при ~275 Вт (RTX 3070 —
~20 FW/s): CMFD гораздо выгоднее на тензорных ядрах NVIDIA.

**Андервольт для CMFD на AMD (с 1.7.1):** уберите `rem` перед `set "AUTO_UV=1"` в `start-cmfd.bat` (запуск от администратора)
или добавьте `--auto-uv` в командную строку CMFD. `amduv.exe` хранит **отдельный результат для CMFD**: CMFD считает неверно на
частотах, где Pearl ещё работает (RX 6800 XT на минимальном напряжении: Pearl проходит до 2250 МГц, CMFD ошибается уже с
2200–2250), поэтому точка Pearl для CMFD не используется. Каждая точка проверяется на настоящей модели: OpenCL-воркер снова и
снова считает фиксированные пачки по 32 nonce, и их финальные дайджесты (BLAKE3 по всем 384 слоям) должны побитно совпасть со
снятыми на стоке — один неверный бит где угодно их меняет. Первый запуск подбирает (~15–20 мин, один раз на карту / драйвер /
воркер / модель), следующие применяют сохранённую точку (проверка 15 с, если прошлая была больше 6 ч назад). Сторож **возвращает
ваши настройки частоты / напряжения / лимита мощности**, когда майнер завершается, в том числе после падения или kill.
`UV_GOAL` в `start-cmfd.bat` (`$FFGP_AMD_UV_GOAL`) выбирает один из подобранных профилей:

| Профиль | Как находится | RX 6800 XT (Вт чипа, ADLX) |
|---|---|---|
| — (сток) | | 10,0 FW/s при 280 Вт (36 FW/кДж) |
| `eff` | минимальное напряжение, предел частоты вверх до ошибки проверки, шаг назад | 2100 МГц / 881 мВ: 9,1 FW/s при 193 Вт (47 FW/кДж, **+32 %**; −9 % FW/s) |
| `bal` (по умолчанию) | больше всего FW на ватт при ≥ 90 % стоковых FW/s | на этой карте = `eff` |
| `max` | ваша макс. частота, напряжение вниз до ошибки проверки, шаг обратно вверх | 2324 МГц / 1075 мВ: 10,1 FW/s при 248 Вт (41 FW/кДж, +14 %; скорость та же) |

На пуле Aria через майнер (по 10–15 мин, среднее по строке статистики): сток 9,9 FW/s при 277 Вт, `bal` 9,1 FW/s при 191 Вт
(+33 % FW/Вт), `max` 10,0 FW/s при 247 Вт (+14 %); отклонённых шар нет, у движка `INTEGRITY | ok`. Край немного плавает от
прогона к прогону (2200 МГц при 881 мВ прошли дважды и один раз нет), поэтому тюнер всегда оставляет шаг запаса.

Бесполезно в Windows: лимит мощности опускается только до −6 % (264 Вт, +4 % FW/Вт); частоту памяти можно только поднять
(2000–2150 МГц: прироста нет, а на 2150 — неверные результаты), быстрые тайминги памяти ничего не меняют. `--uv-reset` забывает
результаты Pearl и CMFD (следующий запуск с `--auto-uv` подберёт заново).

### 15. NOID (Parano1d), новое в 1.7.0

FreeForGem майнит **NOID** (Parano1d: Poseidon2b над GF(2^128), proof of work) на видеокартах NVIDIA своими ядрами (закрытый
код, `libnoidgpu.so` / `noidgpu.dll`, отдельно от библиотеки Pearl). Комиссия разработчика — **1 %** (один слот 60 с за каждые
100 минут, на вашем пуле), как у Pearl. Каждый кандидат, найденный GPU, перед отправкой пересчитывается на CPU официальным
крейтом `noid_poseidon2b`.

**Требования**

* NVIDIA **RTX 30 / RTX 40 / RTX 50** или A100/H100 (compute capability 8.0+). RTX 20 / GTX 16 / CMP (Turing) и AMD для NOID
  **не** поддерживаются.
* Драйвер NVIDIA **580 или новее** (ядра используют инструкцию `clmad` из CUDA 13). Со старым драйвером майнер останавливается
  с сообщением `NOID needs an NVIDIA driver R580 or newer`. HiveOS: `nvidia-driver-update 580` (или новее).
* Адрес кошелька NOID (`o1...`) и NOID-пул. NOID-пулы работают по `parano1d-stratum-v1` через **TLS**:

| Пул | URL | Примечание |
|---|---|---|
| InnovLab (EU) | `stratum+ssl://eu2.innovlab.cc:19601` | проверен с FreeForGem (шары приняты) |
| Suprnova | `stratum+ssl://noid.suprnova.cc:3340` | TLS-соединение проверено, майнинг не проверялся |

Просто `HOST:PORT` считается TLS; `stratum+tcp://` — обычный TCP; `?insecure` в конце URL разрешает самоподписанный
сертификат пула. Монета выбирается `--algo noid` (или автоматически, если в имени хоста пула есть `noid`, `parano1d` или
`innovlab`). Логин на пуле — `КОШЕЛЁК.ВОРКЕР`.

**Два ядра: `--noid-mode eff` (по умолчанию) и `--noid-mode speed`**

RTX 3070, память 810 МГц, лимит мощности 135 Вт, по 60 с на точку (`eff` — 512 потоков на SM, `speed` — больше S-box на CLMAD;
в обоих — цепочка S-box x·(x·x²)² с общим операндом):

| Частота ядра / offset | eff: MH/s @ Вт (MH/Вт) | speed: MH/s @ Вт (MH/Вт) |
|---|---|---|
| 1400 МГц / +220 | **34.70 @ 73.2 (0.474)** | — |
| 1560 МГц / +205 | 38.40 @ 92.2 (0.417) (offset +175) | 43.65 @ 105.3 (0.415) |
| 1650 МГц / +220 | — | **46.16 @ 121.1 (0.381)** |

* **eff** (по умолчанию, лучшие MH на ватт): лок ядра **1400 МГц** и `--auto-uv`.
* **speed** (максимум MH/s): ядро **1560–1650 МГц** (RTX 30): 43.65 MH/s на 1560 МГц, 46.16 MH/s на 1650 МГц.
* RTX 40/50: те же ядра, собранные под sm_89 / sm_120; частоту подбирайте сами (начните со стоковой и `--auto-uv`).

**Командная строка**

```
./freeforgem --algo noid --pool stratum+ssl://eu2.innovlab.cc:19601 --wallet o1ВАШ_АДРЕС --worker rig1 \
             --noid-mode eff --lock-cclock 1400 --lock-mclock 810 --auto-uv
./freeforgem --algo noid --self-test          # официальные тест-векторы + дайджесты GPU против CPU, оба ядра, все GPU
```

**Полётный лист HiveOS:** Coin `NOID` (любое имя), Wallet `o1...`, Pool: Configure in miner; Miner **Custom** → Miner name
`freeforgem`, Installation URL релиза, Hash algorithm `parano1d`, Wallet and worker template `%WAL%.%WORKER_NAME%`, Pool URL
`stratum+ssl://eu2.innovlab.cc:19601`, Extra config `--algo noid --lock-cclock 1400 --auto-uv`. Хэшрейт в панели — в kH/s
(MH/s × 1000).

**Windows:** отредактируйте `start-noid.bat` (WALLET, POOL, MODE, при желании AUTO_UV=1 и LOCK_CCLOCK; для AUTO_UV и частот —
запуск от имени администратора) и запустите его.

**Андервольт для NOID.** NOID нужен свой offset: андервольт Pearl для него обычно слишком глубокий (RTX 3070: Pearl-овский
+250 давал неверные результаты NOID; RTX 4070 Ti: Pearl-овский +300 владельца роняет NOID). Поэтому:

* без `--auto-uv` **положительный offset ядра ставится в 0** на время NOID и возвращается при выходе;
* с `--auto-uv` FreeForGem подбирает NOID-offset для каждой карты **и каждого режима** (почему каждый шаг занимает 40 с —
  раздел 16): проверка базового offset, затем от `+120` (`--uv-min`) вверх шагами по 15 МГц (`--uv-step`) до предела карты
  (`--uv-max`, те же умолчания по картам, что у Pearl); первый шаг с ошибкой — край, результат на шаг ниже последнего хорошего,
  подтверждение 2 минуты. Первый запуск — около 10 минут, следующие применяют сохранённый результат (проверка 30 с, если он
  старше `--uv-reval-hours`). Результат хранится в `uv.json` в разделе `noid` (результаты Pearl и CMFD не трогаются) вместе с
  драйвером, ядром, локом частоты и лимитом мощности: если что-то из этого поменялось — карта подбирается заново;
* `--uv-offset N` ставит N без проверок (на ваш риск); `--uv-reset` забывает результаты NOID;
* в майнинге каждый кандидат перепроверяется на CPU, и каждые 120 с на каждой GPU идёт короткая сторожевая проверка (6 запусков
  фиксированного задания, побайтно); ошибка откатывает offset этой карты на шаг (с сохранением), майнинг продолжается;
* применение разгона во время майнинга (`nvidia-oc` в HiveOS при старте майнера и после смены OC пишет *Core Clock* профиля,
  0 при пустом поле; offset от Pearl там даёт тихо неверные результаты NOID): offset перечитывается каждые 30 с, подобранный
  NOID-offset возвращается после 30-секундной стресс-проверки с паузой майнинга на этой карте (при ошибке — шаг назад), база 0
  (без `--auto-uv`) или значение `--uv-offset` ставится сразу; не чаще 4 раз в час, любое другое значение — майнер отступает
  на этой карте (с 1.7.1). Проверка, во время которой разгон сбросил offset, повторяется;
* RTX 3070, наши прогоны: **eff @1400 МГц** — 87.5 Вт на offset 0 → **72.8 Вт на +195** (34.3 MH/s; пол напряжения: выше
  +195 мощность уже не падает, тюнер там останавливается), MH/Вт **+20 %**; на пуле 34.8 MH/s @ 73 Вт (0.476 MH/Вт), 41/41 шар
  принято. **speed @1560 МГц** — 127.8 Вт → **106.2 Вт на +195** (43.3 MH/s), край +225, MH/Вт **+20 %**; на пуле 43.8 MH/s
  @ 107 Вт (0.410), 24/24 принято. RTX 4070 Ti на стоковых частотах и лимите 190 Вт, offset 0: speed 82.6 MH/s, eff ~81 MH/s
  (обе упираются в лимит мощности; тюнинг ещё не делали).

**Лог (NOID)**

```
FreeForGem 1.7.2-beta | NOID (parano1d) | pool stratum+ssl://eu2.innovlab.cc:19601 | worker rig1 | 1 GPU(s) [0] | mode eff
[noid-pool:user] subscribed: parano1d-stratum-v1, nonce_bits 64, session namespace 24e67d0000000200
new job 46f6...0000 [user] height 239348 (share target 0000003fffffffff.., expires in 262s)
[  60s] GPU0 34.700 (73 W, 0.474 MH/W) | total 34.700 MH/s, 73 W, 0.474 MH/W | hits 412 (bad 0, stale 0) | user sub 3 acc 3 rej 0 | fee sub 0 acc 0 rej 0
```

`hits` — кандидаты, перепроверенные на CPU (ядро сообщает больше, чем шар: примерно 1 на 4 млн nonce); `bad` — кандидаты или
сторожевые запуски, не прошедшие проверку (неверные результаты GPU: offset откатывается); `stale` — шары задания, которое пул
уже заменил (не отправляются). `pool pause (canonical-tip-unverified)` — норма: пул снимает задание и сразу присылает новое.

### 16. «Тихий край» андервольта (почему `--auto-uv` проверяет так много)

При увеличении offset ядра GPU получает меньшее напряжение на той же частоте. Далеко от предела всё в порядке. Вблизи предела
карта **не** падает, драйвер **не** сообщает об ошибках (Xid нет), быстрый self-test **проходит** — но часть результатов уже
**неверна**. Наш замер на NOID, RTX 3070 на 1560 МГц: на +250 **168 из 5984** полных запусков вернули неверный набор хитов,
первый — примерно через 12 с нагрузки, без единой ошибки драйвера; 2-минутный self-test при этом прошёл. На шаг ниже (+235) —
тоже ошибки; +220 — чисто.

Чем это грозит: неверные результаты превращаются в **невалидные шары** или в **шары, которые так и не найдены**. В обычном логе
майнера этого не видно — хэшрейт тот же, просто пул платит меньше. Майнер, который следит только за падениями и ошибками Xid,
настраивается прямо в эту зону.

Как это решает FreeForGem (Pearl, CMFD и NOID — по одному правилу):

1. **Каждый шаг проверяется побайтно под полной нагрузкой, много раз** — а не одним self-test. Pearl: тысячи попыток
   фиксированного задания, дайджест каждого тайла сравнивается с первым прогоном, найденные результаты проверяются на CPU
   официальным верификатором. NOID: длинная stress-проверка — запуски на всю сетку по 40 с на шаг, набор кандидатов каждого
   запуска сравнивается с набором CPU официального крейта (около 4000 запусков на шаг на RTX 3070). CMFD: дайджесты
   фиксированных батчей на настоящей модели 6.4 ГБ сравниваются с эталоном на стоке (в 1.7.2-beta CMFD работает с offset 0;
   тюнер CMFD выходит отдельно).
2. **Первый шаг с ошибкой — это край.** Результат берётся **на шаг ниже** (запас 15 МГц; `--uv-keep-last` оставляет последний
   хороший шаг) и **подтверждается** 2-минутным прогоном; если подтверждение не прошло — ещё на шаг ниже.
3. **В майнинге результаты продолжают проверяться:** каждый хит Pearl и каждый кандидат NOID пересчитывается на CPU перед
   отправкой; у NOID ещё сторожевая проверка каждые 120 с; Pearl перепроверяется каждые 6 часов. Неудачная проверка
   **автоматически откатывает offset на шаг** (с сохранением), майнинг продолжается.
4. Offset владельца записывается в файл восстановления до первого изменения и **возвращается при выходе** (а после падения — при
   следующем запуске).

Как это выглядит в логе (ядро speed, RTX 3070, `--lock-cclock 1560 --auto-uv`, наш прогон; две последние строки — из прогона с заведомо слишком глубоким `--uv-offset 280`):

```
auto-uv (noid): GPU0 (bus 0d, RTX 3070) tuning the NOID offset (speed kernel speed:aff/sm86): base +0, start +120, limit +495 (default), step 15, 40 s of stress per step
auto-uv (noid): GPU0 (bus 0d, RTX 3070) base +0: ok (3448 launches checked, 43.29 MH/s, 127.8 W, 0.339 MH/W, 1560 MHz, 56 C)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) +120: ok (4597 launches checked, 43.30 MH/s, 114.4 W, 0.378 MH/W, 1560 MHz, 56 C)
...
auto-uv (noid): GPU0 (bus 0d, RTX 3070) +210: ok (4598 launches checked, 43.30 MH/s, 103.8 W, 0.417 MH/W, 1560 MHz, 55 C)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) +225: FAILED (WRONG RESULTS: 5 of 4597 launches gave a wrong candidate set, first after 14.1s (no Xid, no crash: the silent edge)): edge found
auto-uv (noid): GPU0 (bus 0d, RTX 3070) confirmation +195: ok (13790 launches checked, 43.30 MH/s, 106.2 W, 0.408 MH/W, 1560 MHz, 57 C)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) RESULT speed: offset +195 (edge +225, one step below the last good one); MH/W 0.339 -> 0.408 (+20.3%); saved to .../uv.json
...
GPU0: WATCHDOG: 1 of 6 check launches gave WRONG results (silent undervolt edge?)
auto-uv (noid): GPU0: wrong GPU results at offset +280 (CPU check of a hit / watchdog): stepped back to +265 and mining on
```

### 17. Профили по локу частоты, майнинг во время тюнинга, падения в майнинге (новое в 1.7.2)

**Зачем.** Край андервольта зависит от частоты ядра: offset, проверенный на 1410 МГц, небезопасен на 1805 МГц. На риге 8x RTX 3070
(HiveOS) другой майнер оставил лок 1400 МГц, auto-uv нашёл там +450..+495, а после возврата лока HiveOS 1805 МГц эти offset'ы были
«применены без стартовой проверки»: illegal memory access, Xid 13/43, цикл перезапусков. В 1.7.2:

**Профиль на каждый лок.** auto-uv хранит отдельный результат для каждой карты, монеты / режима NOID и лока ядра (`uv.json`,
секции `profiles`, `noid`, `cmfd_profiles`; лок памяти входит в ключ, если его ставит майнер, например `1805/mem810`):

* с `--lock-cclock N` лок известен и выбирает свой профиль; переключение 1805 -> 1410 -> 1805 сразу берёт проверенный результат
  каждого лока (без проверки, если проверен в пределах `--uv-reval-hours`, иначе после обычной быстрой проверки); лок без профиля
  тюнится, профили других локов не трогаются;
* без `--lock-cclock` майнер не может прочитать лок (в NVML нет такой функции; его могли сменить HiveOS или другой майнер):
  майнинг стартует на вашем offset, частота под нагрузкой (~20 с) выбирает профиль: `1805`, `~1815` (лок поставлен не майнером)
  или `unlocked` (частота «плавала» при тюнинге); подходящего нет — карта тюнится для этих частот. NOID и CMFD меряют частоту
  короткой проверкой на базовом offset;
* стартовый лог показывает профили каждой карты: `profiles: 1400:+210, 1900:+180; using 1400 for the core clock lock 1400 MHz`;
* `--uv-reset` забывает только профиль частот (и монеты / режима) этого запуска, остальные остаются; `--uv-reset-all` — все;
* результат до 1.7.2 (один на карту, лок не записан) становится профилем частоты, на которой найден (`~1410`), и используется
  только после проверки на частотах запуска (CMFD: лок из идентификатора движка становится ключом); результаты NOID сохраняются,
  только если тюнились с `--lock-cclock`;
* в майнинге рост частоты под нагрузкой при том же offset (лок подняли) возвращает ваш offset, и профиль заново выбирает частота;
  после OC-apply без `--lock-cclock` тоже сначала решает частота.

**Майнинг во время тюнинга.** До 1.7.2 ни одна карта не майнила, пока не дотюнятся все (~25 мин на риге из 8 карт). Теперь майнинг
стартует сразу; проверки и тюнинг каждой карты идут в фоне и ставят на паузу только её (на время её тюнинга или быстрой проверки); остальные
карты майнят. Очередь
проверок по RAM («waits for RAM») не останавливает майнинг ждущей карты.

**Ваш offset.** На HiveOS при выходе возвращается значение *Core Clock* профиля разгона этой карты (`/hive-config/nvidia-oc.conf`),
а не то, что было на карте при старте (Rig01: +325 — остаток прерванного ffgs вместо +275 профиля); такой остаток при старте
заменяется значением профиля. Результат, проверенный на этих частотах ниже вашего offset, **используется вместо него** (ваш offset
там за краем); вслепую он никогда не оставляется. Если не проходит сам стартовый offset тюнинга, ищется меньший (шагами 60 МГц),
и он сохраняется как профиль. `WARNING`: одно значение `--lock-cclock` на все карты заменяет поканальные локи HiveOS.

**Какая карта упала.** Сбой CUDA на одной карте (Xid 13 «Out Of Range Address», Xid 43, «an illegal memory access») завершает CUDA
**всего процесса майнера**: все воркеры останавливаются с той же ошибкой, в том числе на картах без Xid. Теперь майнер:

1. определяет упавшую карту по событиям Xid драйвера (NVML): `faults: GPU3 (bus 04, RTX 3070) FAILED: Xid 13 ...; the workers of
   GPU0, GPU1, ... stopped with it: ... they are not at fault`;
2. запоминает её в `gpu-faults.json` рядом с `uv.json` (падений подряд, Xid, offset в момент падения);
3. перезапускает себя (Linux / HiveOS — в том же процессе, HiveOS не считает перезапуск; не больше 12 раз в час, дальше выходит,
   как раньше; Windows — перезапускает start.ps1);
4. с `--auto-uv`: упавший андервольт-offset считается провальным, карта тюнится ниже; падение на вашем / оставленном offset
   **2 старта подряд** понижает его (`CRASHED while mining at +325 2 starts in a row ...: running it at +275`): сначала до Core Clock
   профиля HiveOS, затем шагами по 60 МГц до 0; ваш новый offset отменяет понижение;
5. карта, которая падает и дальше, когда понижать некуда (без `--auto-uv` — после 2 стартов подряд с падением), **исключается из
   майнинга на час**, остальные майнят. 30 мин без сбоя обнуляют счёт; `--uv-reset` не очищает `gpu-faults.json` (удалите файл).

---

## 中文

### 1. 这是什么

FreeForGem 是面向 NVIDIA 显卡的 **Pearl (PRL)**（pearlhash，有用工作量证明）GPU 矿工，开发者抽水 **1%**。
自 1.6.0 起也可挖 Common Foundry（CMFD，第 13 节），自 1.7.0 起可挖 **NOID**（Parano1d，第 15 节）。

**名称。** 自 1.7.0 起，多币种矿工（Pearl + CMFD + NOID）更名为 **FreeForGem**（程序 `freeforgem`，HiveOS 自定义矿工
`freeforgem`，代码仓库 `pepsykolya/FreeForGem`）。**FreeForGemPear** 作为 Pearl 矿工以原名继续存在。从 FreeForGemPear 迁移：
在 HiveOS 中创建**新的飞行表**（Miner name `freeforgem` + 新的 Installation URL）；已保存的降压结果（`/hive-config/freeforgempear`
或新文件夹旁边的 `freeforgempear` 文件夹中的 `uv.json`）会在首次启动时自动导入（日志显示
`imported the saved undervolt results of FreeForGemPear`）。
它使用针对 Ampere 的自研 CUDA 内核：A′ 矩阵在芯片上生成，int8 张量核心 GEMM 在常驻内核中运行，jackpot 折叠和 BLAKE3
哈希都融合在其中。主循环在 SASS 级别手工调优。提交之前，主机会用官方 `zk-pow` 验证器构建并检查每个份额的证明。

| 显卡系列 | 状态 |
|---|---|
| RTX 30xx（Ampere，sm_86）：3060 / 3060 Ti / 3070 / 3070 Ti / 3080 / 3090 | **主要目标，已调优：RTX 3070 约 70 TH/s，约 107 W** |
| RTX 40xx（Ada，sm_89） | 自 1.5.4 起使用专用 SASS 内核（RTX 4070 Ti：算力相同，功耗比 RTX 30xx 内核低约 10 W） |
| RTX 50xx（Blackwell，sm_120） | 自 1.5.0 起支持，使用编译内核（未调优）；需要驱动 570+ |
| CMP 50HX（TU102） | 可以运行，但**不建议用于 Pearl：每卡约 2.6 TH/s**（int8 张量核心相比 CMP 40HX 被削减约 17 倍，与驱动无关） |
| RTX 20xx / GTX 16xx / CMP 30HX–50HX（Turing，sm_75） | 自 1.5.3 起支持，专用 SASS 内核：RTX 2060 SUPER @1605 MHz 约 49.9 TH/s；**显存必须锁定 5001，不是 810** |
| AMD Radeon RX 6000（RDNA2），Windows | **1.6.0 新增**，自有 OpenCL 内核：RX 6800 XT 上 34.4 TH/s（第 14 节） |
| 更老的显卡、A100（sm_80）、Linux / HiveOS 上的 AMD | 不支持 |

### 2. 系统要求

* NVIDIA 显卡：RTX 30 / RTX 40（专用内核）、RTX 20 / GTX 16 / CMP、RTX 50（自 1.5.0 起）；NVIDIA 驱动 **525 或更新**（RTX 50xx：**570+**）。
* 或者（**仅限 Windows**）使用 **AMD Radeon RX 6000**（RDNA2）显卡及 AMD Adrenalin 驱动（自 1.6.0 起，见第 14 节）。
* HiveOS、Linux x86_64（glibc 2.31+，Ubuntu 20.04+）或 Windows 10/11 x64。不需要 CUDA toolkit、Python 或 WSL。
* PRL 钱包地址（`prl1...`）以及到 Pearl stratum 矿池的网络连接。
* 显存：内核从 L2 缓存工作，几乎不占用显存。

### 3. HiveOS 设置（分步说明）

FreeForGem 在 HiveOS 中作为 **Custom miner** 运行：无需 SSH，无需手动安装。

**安装链接 (1.4.9)：**

```
https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.2-beta/freeforgem-1.7.2beta.tar.gz
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
   | Miner name | `freeforgem`（粘贴链接后自动填入） |
   | Installation URL | 上面的链接 |
   | Hash algorithm | `pearlhash`（或保持 `----`） |
   | Wallet and worker template | `%WAL%.%WORKER_NAME%` |
   | Pool URL | `pearl-eu2.luckypool.io:3360`，或 Kryptex `prl-eu.kryptex.network:7048`（第 6 节中的任一矿池） |
   | Pass | 留空 |
   | Extra config arguments | 可选，例如 `--devices 0,1` |

4. **应用。** *Apply changes*，*Create flight sheet*，然后应用到矿机。

矿工日志位于 `/var/log/miner/freeforgem/freeforgem.log`，也可在 HiveOS 矿工控制台查看。
仪表盘显示每块显卡和总算力、温度、风扇，以及已接受和被拒绝的份额。更新时把 Installation URL 改成新版本即可。
更详细的同一指南：[HIVEOS.md](HIVEOS.md)。

### 4. 不使用 HiveOS 的 Linux

```
tar xzf freeforgem-1.7.2-beta-linux-x86_64.tar.gz
cd freeforgem-1.7.2-beta
./freeforgem --self-test        # 可选：将 GPU 内核与 CPU 参考实现比对
./freeforgem --pool pearl-eu2.luckypool.io:3360 --wallet 你的PRL地址 --worker rig1     # LuckyPool
./freeforgem --pool prl-eu.kryptex.network:7048 --wallet 你的PRL地址 --worker rig1       # Kryptex
```

| 选项 | 含义 |
|---|---|
| `--pool HOST:PORT` | stratum 矿池（默认 `pearl-eu2.luckypool.io:3360`）；可带 `stratum+tcp://` 前缀，不支持 SSL（请使用普通 TCP 端口） |
| `--wallet ADDRESS` | 你的 PRL 地址（必填） |
| `--worker NAME` | 矿工名（默认 `worker1`） |
| `--devices 0,1,...` | 使用的显卡（默认所有可见显卡；遵循 `CUDA_VISIBLE_DEVICES`） |
| `--minutes N` | N 分钟后停止（默认一直运行到 Ctrl-C / SIGTERM） |
| `--self-test` | 验证内核后退出 |

一个进程驱动所有显卡，并自动重连矿池。长期运行请放在 `systemd`、`screen` 或 `tmux` 下。

### 5. Windows 10/11

原生 Windows 版本：无需 WSL、无需安装 CUDA、无需 Python。只需要 NVIDIA 驱动。

1. 从发布页面下载 **`freeforgem-1.7.2-beta-windows-x64.zip`**：
   `https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.2-beta/freeforgem-1.7.2-beta-windows-x64.zip`
   并核对 SHA-256（第 9 节）。
2. 解压到**不含空格和非英文字符**的文件夹，例如 `C:\FreeForGem`。
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
* 不使用 `start.bat`：`freeforgem.exe --pool pearl-eu2.luckypool.io:3360 --wallet 你的PRL地址 --worker rig1`
* 自行编译：`.\build-windows.ps1`（见 README）。

**进阶：使用 `nvidia-smi` 锁频 + 功耗限制的自定义 `.bat`。** 如果你还需要**功耗限制**（内置频率控制不设置功耗限制），或想要自己的重启循环和日志文件，可以写一个如下的 `.bat`，并**以管理员身份**运行（`nvidia-smi -lgc/-lmc/-pl` 和 `--auto-uv` 都需要管理员权限）：

```bat
@echo off
rem 通过驱动锁定频率和功耗限制（需管理员；重启后失效）。数值见第 7 节。
nvidia-smi -lgc 1665
nvidia-smi -lmc 810
nvidia-smi -pl 165
cd /d C:\Mining\freeforgem-1.7.2-beta-windows-x64
:loop
powershell -NoProfile -Command "& { .\freeforgem.exe --pool pearl-eu2.luckypool.io:3360 --wallet 你的PRL地址 --worker rig1 --auto-uv --uv-min 135 --uv-max 360 | Tee-Object -Append -FilePath C:\Mining\ffgp.log }"
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

**FreeForGem 实测，RTX 3070：**核心锁定 `1560` MHz，显存 `810` MHz，功耗限制 `120` W →
**约 70 TH/s，约 105–107 W**；随后 `--auto-uv` 找到 +195..240（约 86–107 W）。

**各显卡起始值。***参考*行是这些显卡已公开的调校结果；*估计*行根据同代显卡推算，未经实测。每块卡都不同：让 `--auto-uv` 找出你自己的偏移。

| 显卡 | 核心锁定，MHz | 显存 | V/F 偏移，MHz | 来源 |
|---|---|---|---|---|
| RTX 3060 Ti、RTX 3070、RTX 3070 Laptop、RTX 3080 Ti | 1560–1650 | 锁定 `810` | +200..255 | 参考（3070 为实测） |
| RTX 3060、RTX 3070 Ti、RTX 3080、RTX 3090 | 1560–1650 | 锁定 `810` | +200..255 | 估计 |
| CMP 70HX、CMP 90HX（Ampere） | 1650 | 偏移 `-2000`（`--moffset`） | +255..300 | 参考，未用 FreeForGem 验证 |
| RTX 4070 Ti、RTX 4090 | 2445 | 锁定 `810` | +315..345 | 参考 |
| RTX 4060、4060 Ti、4070、4070 Super、4070 Ti Super、4080、4080 Super | 1900–2445 | 锁定 `810` | +300..345 | 估计 |
| RTX 5070 Ti | 2445 | 锁定 `810` | +480 | 参考 |
| RTX 5070 | 2400 | 锁定 `810` | 检查通过至 +390，更高则崩溃 | 用户报告，FreeForGemPear 1.5.1，Windows，驱动 617.14 |
| RTX 5060、5060 Ti、5080、5090 | 1900–2500 | 锁定 `810` | +390..480 | 估计 |
| RTX 2060 SUPER、RTX 2070/2080（含 SUPER）、GTX 1660/1650 | 1500–1650 | 锁定 `5001`（**不是 810**：Turing 在 810 下损失约 25%） | +200..260；自 1.6.2 起 auto-uv 默认上限 +225 | 参考（2060 SUPER：实测） |
| CMP 40HX | 1400 | 锁定 `5001` | +150；自 1.6.2 起 auto-uv 默认上限 +150 | 用户矿机；CMP 40HX 在 +255 出现 Xid |
| CMP 50HX（TU102） | — | 锁定 `5001` | +220 及以上；auto-uv 默认上限 +225（`--uv-max 225`） | 用户矿机。**不建议用于 Pearl：每卡约 2.6 TH/s**（int8 张量核心相比 40HX 被削减约 17 倍；与驱动无关：550.78 与 595.91.07 相同，3× CMP 50HX 矿机实测） |

RTX 40 和 50 的锁定核心频率高得多（**1900–2500 MHz**），能承受的偏移也比 RTX 30 更高；因此 auto-uv 在 RTX 40 上从 +200、在 RTX 50 上从 +300 开始搜索（RTX 30 从 +120）。

**混合矿机：**只需 `--auto-uv`——上限按每块显卡自动选择（自 1.6.2 起）：RTX 20xx / GTX 16xx 最高 +225，CMP 30HX/40HX
最高 +150（CMP 50HX：+225），其他最高 +495；启动时日志会显示每块显卡的范围（`auto-uv: GPU5 (bus 0b, CMP 40HX): range +120..+150 (max: CMP default; start: default)`）。
如需手动控制，按 `--devices` 顺序每卡一个值：`--uv-max 225,495,150`（`-` = 该卡的默认值）。
Turing（RTX 20 / GTX 16 / CMP 30HX–50HX，sm_75）自 1.5.3 起支持：与 RTX 30 不同，它需要将显存锁定在 5001（见表）；CMP 170HX / A100（sm_80）不受支持。

**CMP 显卡（CMP 30HX / 40HX / 50HX）。** CMP 30HX/40HX 使用 Turing 内核挖 Pearl（显存锁定 5001，auto-uv 上限 +150）。**CMP 50HX
不建议用于 Pearl**——每卡仅约 2.6 TH/s（int8 张量核心相比 CMP 40HX 被削减约 17 倍，与驱动无关）。**CMP 50HX 挖 Common Foundry
（CMFD）表现良好：** 每卡约 13 FW/s、约 220 W（功耗限制 225 W，核心锁定 1410 MHz，显存不锁定；4× CMP 50HX 矿机，Aria 矿池），
使用 `--auto-uv`（CMFD 调校，+225）约 13.2 FW/s、196–203 W（FW/kWh +8–11 %）；见第 13 节。CMFD 需要约 7 GB 空闲显存，6 GB 显卡
（CMP 30HX）无法运行。**FreeForGem 不提供 CMP 解锁：** 我们不提供也不支持解除 CMP 显卡限制的驱动或工具；这里的所有数据均为显卡原状下的结果。

* HiveOS：Overclock 选项卡 → 核心频率（绝对值）、显存频率（绝对值）、功耗限制；V/F 偏移用 `--auto-uv`。
* Linux：`nvidia-smi -lgc 1560,1560`、`nvidia-smi -lmc 810,810`、`nvidia-smi -pl 120`（root），或 `--lock-cclock` / `--lock-mclock`。
* Windows：在 `start.bat` 中设置 `LOCK_CCLOCK` / `LOCK_MCLOCK` / `MOFFSET`（以管理员身份），需要功耗限制时用 `start-nvidia-smi.bat`，
  或 MSI Afterburner。各显卡的现成组合见第 12 节 *Windows 现成组合*。

如果份额被拒绝或日志出现 `bad` 命中，请降低核心频率或偏移。

### 8. 如何阅读日志

| 行 | 含义 |
|---|---|
| `FreeForGem 1.7.2-beta \| pool ... \| worker ... \| N GPU(s)` | 启动横幅 |
| `[pearlgpu] search kernel: SASS-patched cubin (...)` | 已加载调优内核。`embedded search kernel could not be loaded (原因)` = 矿工报错退出（驱动低于 525 或显卡不受支持） |
| `new job ... prepared in ...` | 新的矿池任务已就绪 |
| `[ 120s] GPU0 ... \| total X TH/s \| hits ... \| user sub S acc A rej R \| fee sub ... acc ... rej ...` | 每 10 秒：每块显卡和总算力；你和抽水的已提交 / 已接受 / 已拒绝份额；抽水时段显示 `[DEVFEE]` 自 1.5.3 起每块显卡还显示功耗和能效，例如 `GPU0 49.906 (151 W, 0.330 TH/W)`，以及总计 `\| 151 W, 0.330 TH/W`（需要 NVML） |
| `[DISCONNECTED]` | 与矿池断开；矿工会自动重连 |
| `devfee: ...` | 抽水时段（第 10 节） |
| `!! ...` | 警告或错误（GPU 工作线程停止、某个 hit 未提交） |

### 9. 校验下载（SHA-256）

将结果与发布页面上的哈希比对。

* Linux：`sha256sum freeforgem-1.7.2-beta-linux-x86_64.tar.gz freeforgem-1.7.2beta.tar.gz`
* Windows：`certutil -hashfile freeforgem-1.7.2-beta-windows-x64.zip SHA256` 或
  `Get-FileHash freeforgem-1.7.2-beta-windows-x64.zip -Algorithm SHA256`

### 10. 开发者抽水 — 1%

每 100 分钟一个 60 秒时段，位于周期内的随机时刻，在**你配置的同一个矿池**上，使用单独的登录（矿工名 `ffgp-devfee`）。
日志显示 `devfee: next slot in N min`、`devfee: mining 60s for FreeForGem dev (1%)` 和 `devfee: back to user`。
第一个时段出现在最初 100 分钟内的随机时刻。如果抽水登录无法服务，该时段会被跳过；你的挖矿不会被阻塞。
自 1.4.2 起，在 LuckyPool 上抽水登录同时为开发者合并挖 NOCK（`prl1...+NOCK`）；若矿池拒绝该登录，矿工会退回普通的 PRL 抽水钱包。抽水时间仍为 1%。
抽水是许可证的条件：不得移除、降低或重定向（见 [LICENSE](LICENSE) 第 3 条）。

### 11. 故障排除

| 症状 | 解决办法 |
|---|---|
| 检查一切是否正常 | `freeforgem --self-test`（Windows：`freeforgem.exe --self-test`）应以 `== self-test PASSED ==` 结束 |
| 启动时出现 `embedded search kernel could not be loaded` | 更新 NVIDIA 驱动（525+；**RTX 50xx 需要 570+**），并确认显卡为 Ampere（RTX 30xx）或更新 |
| CPU 占用高 | 默认 GPU 等待为休眠（每块显卡远低于一个核心的 10%）。`PEARL_SPIN=1` 会改为忙等（每块显卡占满一个核心）：确认没有设置 |
| `pg_init(...)` 或显存不足错误 | 其他程序占用了显卡（游戏、本地 AI 模型）。关闭它，或用 `--devices` 选择显卡 |
| 长时间 `[DISCONNECTED]` | 检查矿池主机/端口和防火墙（出站 TCP）；试试第 6 节的其他区域 |
| 大量被拒绝的份额 | 降低核心频率（第 7 节），检查散热 |
| 几分钟后算力下降，任务管理器显示 *Video Encode* 负载 | 屏幕共享、录屏或直播（Discord 屏幕共享、OBS、NVIDIA App / 即时重放）在同一块 GPU 上运行并占用算力；请停止它，或在该程序中关闭硬件加速 |
| HiveOS 提示 `Already installed` 且没有变化 | Installation URL 必须指向新版本 |
| Windows：窗口一闪而过 | 在 `cmd` 中运行 `start.bat` 查看信息；路径不得含空格和非英文字符 |
| Windows：`.exe` 被 Defender / SmartScreen 拦截 | 点击"更多信息 → 仍要运行"，或将文件夹加入排除项；核对 SHA-256 |

环境变量（启动矿工前设置）：`PEARL_SPIN=1`（忙等代替休眠）。
`FFGP_M=N` 设置每次尝试的 A 行数（须为 128 的倍数）。`m` 默认值为 47104，矿工会根据每块 GPU 的 SM 数自动选取 `m`（`m = 128 * SM * r`），因此混合 SM 的矿机能让每张显卡各取所需——按 SM 分组各建一个任务（1.4.7 起）。仅在实验时才设置 `FFGP_M`：它对所有设备统一 `m`；`m` 越大算力略高，但会占用更多主机内存。
Windows：在 `cmd` 窗口中 `set PEARL_SPIN=1`，然后在同一窗口中运行 `start.bat`。

### 12. 降压 / `--auto-uv`（可选，1.4.0 新增）

**视频教程。** 降压设置的分步视频作为单独文件附在 GitHub 发布页中（Releases → 对应版本 → *Assets*）：英文 `FreeForGemPear-1.5.3-undervolt-EN.mp4`，俄文 `FreeForGemPear-1.5.2-undervolt-RU.mp4`。

**默认关闭。** 启用 `--auto-uv` 后，矿工会为每块显卡找出一个更低的核心电压设置（显卡的 *V/F 偏移*），在该设置下显卡
仍能全部计算正确；挖矿时使用它，退出时恢复你自己的设置。电压更低意味着在相同算力下功耗更少：频率仍保持你锁定的值（第 7 节）。

**它做什么**
1. 读取显卡当前的 V/F 偏移（HiveOS 超频、你自己的脚本等），并在做任何改动*之前*把它写入恢复文件。
2. 先让显卡预热 3 分钟，然后以 **15 MHz** 为步长提高偏移。每一步在单独的进程中检查约 30 秒：同一个任务计算数千次，
   每个结果的每个 tile 逐位比较；比较正常挖矿尝试的命中计数；找到的结果由官方验证器在 CPU 上验证。接近极限时会出现
   驱动不报任何错误的错误结果，因此检查同时在 GPU 和 CPU 上进行。
3. 第一次不一致、CUDA 错误、Xid 错误或卡死就结束爬升。结果为**最后一个正常步长再减一步（15 MHz 余量）**，并用更长的
   检查（约 2 分钟）确认；确认失败则再降一步。最高尝试到 +495（`--uv-max`，最多 +600；1.5.1 之前为 +300，硬上限 +400）；自 1.6.2 起，未指定 `--uv-max` 时 Turing（RTX 20xx / GTX 16xx）
   为 +225，CMP 30HX/40HX 为 +150（CMP 50HX 为 +225）。RTX 40/50 显卡常能稳定在 +450..480。已保存的结果若高于降低后的上限，不会原样使用：显卡在上限处运行
   （先快速检查），已保存的结果保留，待上限再次提高时使用；若已保存的结果*正好*等于旧上限，上限提高后会重新调校。
4. 结果保存在 `uv.json` 中，以 GPU UUID、驱动版本和内核版本为键。之后启动时，经过快速检查（约 30 秒）后直接应用。
   如果驱动、内核版本或你的起始偏移发生变化，则重新调校。
   每次检查在单独的子进程中运行，约需 2.4 GB 内存（`FFGP_UV_CHECK_MB`）：内存较小的矿机上，只有在空闲内存足够时多块显卡的检查才会
   同时进行，其余的等待（日志中的 `waits for RAM`）并随后进行；所有显卡调校或应用完成后才开始挖矿。挖矿期间的复查会先获得内存，
   再暂停该显卡（自 1.7.1 起），因此等待内存的显卡会继续挖矿。
5. 挖矿期间：每 6 小时（`--uv-reval-hours`）重新检查一次（该显卡的挖矿暂停约 30 秒）。如果找到的结果未通过 CPU 验证，
   或出现一批被拒绝的份额，偏移下调一步（不会低于你自己的值）并保存。**超频重新应用（自 1.7.1 起）：** HiveOS 的 `nvidia-oc`
   在每次矿工启动和每次修改超频时写入超频配置中的 *Core Clock* 值，**该字段为空时写入 0**——1.7.1 之前这会让降压失效直到下次重启
   （一台 4× RTX 3070 矿机功耗 604 W 而不是约 440 W）。现在，读回的偏移若为 0、HiveOS 配置值或你自己的起始偏移，就视为超频重新应用：
   auto-uv 在快速检查后重新设置已验证的偏移（该显卡挖矿暂停约 30 秒；失败则与复查一样降一步；每小时最多 4 次），日志
   `the offset was reset to +0 by an OC apply (...); re-applying the verified +285 after a quick check`。检查期间偏移被超频重置时，
   该检查会重做。其他任何值都视为你自己的选择：auto-uv 会在该显卡上退让，不会与之对抗。
6. 退出时（Ctrl+C、SIGTERM、HiveOS `miner stop`、关闭 Windows 控制台窗口、正常结束）在所有 GPU 上恢复你的偏移
   （1.6.1 之前 HiveOS 的 `miner stop` 可能只恢复 GPU0）。如果矿工被强制终止（`kill -9`、崩溃），下次启动会先根据恢复文件
   `uv-restore.json` 恢复你的偏移。重启本身也会重置驱动中的偏移。检查进行中被停止（HiveOS 的 `miner stop` / `miner restart`
   先发送 Ctrl+C，再关闭 screen 会话）不算失败（自 1.7.1 起，CMFD 和 NOID 同样）：正在检查的偏移不会被当作崩溃点，也不会暂停
   auto-uv 24 小时；下次启动照常进行。

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
     `set "UV_MAX="`（留空 = 按显卡：+495，RTX 20xx/GTX 16xx/CMP 50HX +225，CMP 30HX/40HX +150；RTX 40/50 常能稳定在 +450..480，RTX 30 通常
     +200..360）。逗号分隔的列表按 `DEVICES` 顺序为每卡设置一个值，例如 `set "UV_MAX=225,495,150"`。
   * `start-nvidia-smi.bat`：`AUTO_UV=1`，范围在 `UV_MIN` / `UV_MAX`，并通过 `nvidia-smi` 设置功耗限制和锁频。
   * 自己的命令行：`freeforgem.exe --pool ... --wallet ... --auto-uv --uv-max 480`。
4. **起点。** 调校从显卡当前的偏移开始，且不会低于它。MSI Afterburner（或类似工具）中的 *Core Clock* 偏移就是同一个偏移：
   你在那里设置的值就是起点，退出时会恢复。如果挖矿期间 Afterburner 重新应用配置文件（偏移回到你的值或 0），auto-uv 会在快速检查后
   恢复已验证的偏移（自 1.7.1 起）；其他值则会让它在该卡上退让。未设置偏移时，调校从 +120 开始，RTX 40 从 +200、RTX 50 从 +300 开始（或 `--uv-min` / `UV_MIN`）。
5. **等待。** 首次启动时每块显卡调校约 15 分钟（所有显卡并行），完成后才开始挖矿。之后的启动在约 30 秒检查后应用已保存的值。
6. **文件**在矿工目录中：`uv.json`（结果）和 `uv-restore.json`（你原来的偏移）。该目录必须可写，所以不要把矿工放在
   `C:\Program Files`。重新调校：关闭矿工，删除 `uv.json`（或在该目录运行 `freeforgem.exe --uv-reset`），再启动。
   新驱动或新的矿工内核会自动触发重新调校。
7. **查看结果：**`findstr auto-uv logs\*.log`（`start-nvidia-smi.bat` 为 `logs\ffgp.log`）。那块 RTX 3070 的示例：
```
auto-uv: GPU0 (bus 04, RTX 3070): CUDA device 0 -> NVML index 0, PCI 0000:04:00.0, ..., current offset +225 (allowed -1000..1000)
auto-uv: GPU0 (bus 04, RTX 3070) step +255: ok (6560 reruns, cpu ok 6), 94.0 W, 1560 MHz, 58 C
auto-uv: GPU0 (bus 04, RTX 3070) step +270: FAILED (rerun_diff: 256 transcript word(s) differ between reruns of seed 3 ...)
auto-uv: GPU0 (bus 04, RTX 3070) offset +225 -> +240, 98.4 W -> 86.5 W, saved
```
   * `auto-uv paused for this GPU for 24 h`：显卡在找到的偏移上出错；它以你自己的偏移挖矿，24 小时后（或 `--uv-reset` 后）再次调校。
   * `start offset ... leaves no room below the cap`：你的偏移已等于或高于 `UV_MAX`（或括号中显示的显卡默认上限）；如确定显卡能承受更多，请提高 `UV_MAX`。
   * `no safe offset above ... was found`：显卡承受不了比你的值更高的偏移；不做任何修改。
8. **如果电脑卡死或驱动重置**（黑屏、“显示驱动程序已停止响应”）：重启后再次启动矿工——它会先从 `uv-restore.json`
   恢复你的偏移。如果矿工本身在调校某一步时崩溃（窗口显示 `Miner exited (code -1073741819)`），下次启动会把这一步视为失败，只在其下方调校（自 1.5.2 起；之前会重新开始整个搜索并在同一步再次崩溃）。自 1.5.3 起还会跳过已经通过的步骤：预热后直接进行确认（约 5 分钟，而不是完整搜索）。如果某块卡反复出错，请降低 `UV_MAX` 或不要开启 `AUTO_UV`。

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
  降压只需 `AUTO_UV=1`：上限按每块显卡选择（RTX 20xx/GTX 16xx/CMP 50HX +225，CMP 30HX/40HX +150，其他 +495；RTX 40/50 自动从 +200/+300 开始）。
  如需手动控制，`UV_MIN` / `UV_MAX` / `UV_OFFSET` 接受同样顺序的列表，例如 `set "UV_MAX=225,495,150"`（`-` = 该卡默认值）。列表必须每卡恰好一个值。
* **更快调校：**`set "UV_STEP=30"` 使步数减半（结果更粗）。在 RTX 50 上，较高的起点（约 +300）最省时间：显卡反正能承受，搜索直接从那里开始（自 1.6.2 起这是 RTX 50 的默认值）。不要设得过高：如果显卡在 `UV_MIN` 处就未通过检查，则不会调校，保持你自己的偏移（日志会说明；此时请降低 `UV_MIN`）。
* **已知道偏移：**`set "UV_OFFSET=480"`（不开启 `AUTO_UV`）——一次快速检查后直接应用，不搜索。
* **还需要功耗限制：**使用 `start-nvidia-smi.bat`（`CCLOCK` / `MCLOCK` / `PLIMIT`，`UV_MIN` / `UV_MAX` / `UV_STEP`）。
* **更改范围后重新调校：**在一次启动中设置 `set "EXTRA=--uv-reset"`，之后再清空。

**风险。** 降压超过显卡的承受能力可能导致驱动重置或卡死（调校只会走到刚刚检查通过的值并留有余量，但芯片、温度和驱动各不相同）。
卡死后请重启：下次启动会首先恢复你的偏移。风险自负，与任何超频一样。

**启用**
* HiveOS：在飞行表的 *Extra config arguments* 中加入 `--auto-uv`（详见 HIVEOS.md）。
* Linux：`sudo ./freeforgem --pool ... --wallet ... --auto-uv`
* Windows：右键 `start.bat` → *编辑*，删除 `set "AUTO_UV=1"` 前面的 `rem`；**以管理员身份**运行 `start.bat`。

| 选项 | 含义 |
|---|---|
| `--auto-uv` | 查找、应用并监控降压（如上所述） |
| `--uv-offset N` | 快速检查后应用固定偏移 N MHz，不做调校（检查失败则保持你的偏移） |
| `--uv-reset` | 忘记已保存的结果；与 `--auto-uv` 一起使用时重新调校；没有钱包地址时矿工直接退出 |
| `--uv-max N` | 调校尝试的最高偏移（最多 600；默认按显卡：RTX 20xx / GTX 16xx / CMP 50HX 225，CMP 30HX/40HX 150，其他 495；Windows：在 `start.bat` 中 `set "UV_MAX=480"`） |
| `--uv-min N` | 调校的起始偏移，且不会低于该值（默认：HiveOS/驱动中的偏移，否则 120；RTX 40 至少 200，RTX 50 至少 300） |
| `--uv-step N` | 每步调校的 MHz（默认 15） |
| `--uv-keep-last` | 某一步失败后保留最后一个通过的步（默认：最后通过的步再减一步） |
| `--uv-reval-hours H` | 挖矿期间的复查间隔（默认 6，`0` 表示不复查） |
| `--uv-dir DIR` | `uv.json` 和 `uv-restore.json` 的存放位置（也可用 `FFGP_STATE_DIR`） |
| `--auto-uv-dry-run` | 只在日志中说明会改动哪些显卡（CUDA 设备 → NVML 设备、总线地址），不做任何改动 |

`--uv-max`、`--uv-min` 和 `--uv-offset` 可以为所有显卡给一个值，或给逗号分隔的列表——按 `--devices` 顺序每卡恰好一个值
（没有 `--devices` 时：按日志中的 GPU0、GPU1……；HiveOS 上即面板中的 PCI 总线顺序）。列表中的 `-` = 该卡的默认值
（`--uv-offset`：不改动该卡）。长度不对的列表会报错，不会自动补齐。显式指定的 `--uv-max` 按原值使用，即使高于显卡默认上限；
此时日志会给出警告（`WARNING --uv-max +495 is above the recommended +150 for this card (CMP default)`）。2× RTX 2060 SUPER、
3× RTX 3070 Laptop 加一块 CMP 40HX 的示例：`--auto-uv --uv-max 225,225,495,495,495,150`（与默认值相同）。

**GPU 频率控制（可选，基于 NVML；需要 root / 管理员权限，Windows 上可用 — 1.4.9 新增）。** `--lock-cclock N` 将核心频率锁定为 N MHz（类似 `nvidia-smi -lgc N,N`），`--lock-mclock N` 将显存频率锁定为 N MHz（类似 `-lmc`），`--moffset N` 设置显存频率偏移（MHz），可以为负数（需要驱动 555+）。每个选项可对所有 GPU 使用同一个值，也可按 `--devices` 顺序给出逗号分隔的列表（每个设备一个值）。设置在启动时应用，退出时（Ctrl-C / SIGTERM / 崩溃）自动撤销。**自 1.7.1 起，原有的锁频会被恢复而不是移除：** 在 HiveOS 上恢复超频配置中的锁频（*Lock Core Clock* / *Lock Memory Clock*，从 `/hive-config/nvidia-oc.conf` 读取）；其他系统上 NVML 无法读回锁频，可在 `FFG_RESTORE_CCLOCK` / `FFG_RESTORE_MCLOCK` 中指定（FreeForGemPear 的变量名 `FFGP_RESTORE_*` 同样有效；一个值或按显卡的列表，0 = 无锁频），否则与以前一样恢复为默认。日志：`core clock locked to 1500 MHz; lock before: 1470 MHz (HiveOS OC profile), put back on exit`，退出时 `core clock lock 1470 MHz put back`。崩溃或 `kill -9` 后，下次启动会做同样的恢复。Windows 上请在 `.bat` 文件中设置 `LOCK_CCLOCK`、`LOCK_MCLOCK`、`MOFFSET`。示例：`--lock-cclock 1500 --lock-mclock 810 --moffset -200`。

文件默认位置：程序旁边（Windows：矿工文件夹）；HiveOS 上为 `/hive-config/freeforgem`（矿工更新和重启后仍保留）。
**重置：** `freeforgem --uv-reset`（或删除 `uv.json`）。

**日志行**（都以 `auto-uv:` 开头，`grep auto-uv` 即可看到全过程）：
```
auto-uv: GPU0 (bus 0d, RTX 3070): CUDA device 0 -> NVML index 1, PCI 0000:0D:00.0, GPU-0d06..., current offset +150
auto-uv: GPU0 (bus 0d, RTX 3070): range +150..+495 (max: default; start: current offset)
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
   （13、31、43、79 ...）。`freeforgem --uv-offset N` 会应用 N，运行与 `--auto-uv` 相同的检查并用它挖矿，是测试候选值的快捷方式。
4. 从第 2 步重复，直到第一次出现问题，然后从出现问题的偏移回退**两步（30 MHz）**并保留该值。
5. 驱动更新、更换散热器或天气炎热后重新测试。

### 13. Common Foundry (CMFD)，1.6.0 新增

同一个程序也可以挖 **Common Foundry（CMFD，ForgeMatrix V4）**。CMFD 的计算由 **FreeForgeMiner** 引擎完成
（MIT 许可，<https://github.com/pepsykolya/freeforgeminer>，1.2.4 版），放在软件包的 `cmfd/` 文件夹中：Windows 使用原版发布文件；Linux/HiveOS 使用在 Ubuntu 20.04 上由同一固定源码编译的
文件，可在所有 HiveOS（Ubuntu 20.04 和 22.04 镜像）上运行。
FreeForGem 负责准备模型、为每块 GPU 启动一个引擎进程、应用您的频率设置、打印常规统计行，并在引擎退出时重新启动。

**币种由矿池地址决定**：`cmfd+tls://` 地址即为 CMFD（`--algo cmfd|pearl` 可强制指定）。钱包为您的 CMFD 地址，两种形式均可：`cmfd1...`（63 个字符，带校验和，Common Foundry 1.0.16+ 钱包显示的格式）
或旧的 64 位十六进制——它们是同一个密钥。`cmfd1...` 地址会被校验（输错会在开始挖矿前被拒绝），并以十六进制发送给矿池。
测试网 `tcmfd1...` 地址不被接受。

**系统要求。** NVIDIA RTX 20xx 或更新（针对 RTX 30/40 优化），**每张卡约 7 GB 空闲显存**（仅模型就有 6.4 GB），
Linux/HiveOS 上 NVIDIA 驱动 R575+，磁盘需 7 GB 存放模型。Linux/HiveOS 需要 glibc 2.30+（Ubuntu 20.04 或更新，所有 HiveOS 镜像）。

**模型**（6.4 GB）首次从 Common Foundry 官方服务器分块下载并校验，之后每次启动只做校验（10–40 秒）。位置：`--cmfd-model DIR`；
HiveOS 上默认 `/hive/miners/custom/cmfd-model`（与 FreeForgeMiner 共用，已运行过它的矿机不会重复下载），否则为程序旁的 `cmfd-model`。

**矿池**（地址连同证书 pin，须与下表完全一致；括号内为矿池费率）：

| 矿池 | 地址 |
|---|---|
| **Aria / AriaBrain**（默认，pool.ariabrain.com/cmfd.html，3 %） | `cmfd+tls://159.69.194.46:29445?pin=9dfb51083f287726117f689f87bc7a878792efcca58ca8b6f6e7c05ac5d152e9` |
| **Midas Mining**（midasmining.net，1 %） | `cmfd+tls://24.107.152.151:29745?pin=2ee4e6a028b07b300753dff3c260b79763c5e748d4e78b86cf3855e97eb2e80e` |
| cmfd-pool.online（3 %） | `cmfd+tls://109.199.124.187:29465?pin=ebe88f5e05f3a222208d551d05b6d39057b64ce8239ba7a708d487e15ac711be` |

地址和 pin 由矿池自己公布（Midas：`https://midasmining.net/api/v1/pool` 的 `public_pool_url` 字段）。pin 变了说明证书变了：
请从矿池获取新地址，切勿关闭 pin。部分矿池（cmfd-pool.online）在大量被拒份额或频繁重连后会封禁 **IP 地址**约一小时，
不要在这些矿池上测试不稳定的超频。

**Linux：**
```
./freeforgem --pool 'cmfd+tls://159.69.194.46:29445?pin=9dfb51083f287726117f689f87bc7a878792efcca58ca8b6f6e7c05ac5d152e9' \
    --wallet cmfd1您的地址 --worker rig1
# Midas Mining：
./freeforgem --pool 'cmfd+tls://24.107.152.151:29745?pin=2ee4e6a028b07b300753dff3c260b79763c5e748d4e78b86cf3855e97eb2e80e' \
    --wallet cmfd1您的地址 --worker rig1
./freeforgem --algo cmfd --self-test        # GPU 自检（无需矿池）：与参考路径逐位比较；若已有模型，
                                                # 还会运行 Common Foundry 1.0.16 官方测试向量（32 个 nonce，SHA-256）
```
在 shell 中请给地址加引号（`?` 是通配符）。如需由矿工自动清除降压偏移（见下文），请以 root 运行。

**HiveOS**（飞行表，Miner 选 *Custom*，与 Pearl 相同的软件包）：*Hash algorithm* `forgematrix_v4`，*Wallet and worker template*
`%WAL%.%WORKER_NAME%`，钱包填您的 CMFD 地址，*Pool URL* 填表中一行，例如
`cmfd+tls://159.69.194.46:29445?pin=9dfb51083f287726117f689f87bc7a878792efcca58ca8b6f6e7c05ac5d152e9`
（Midas：`cmfd+tls://24.107.152.151:29745?pin=2ee4e6a028b07b300753dff3c260b79763c5e748d4e78b86cf3855e97eb2e80e`）。
*Extra config*（可选）：`--cmfd-mode eco`、`--devices 0,1`、`--per-gpu-workers`、`--lock-cclock 1560`。
HiveOS 按卡显示 FW/s；矿机首次启动会下载模型（请查看矿工日志）。
请为 CMFD 飞行表使用**单独的、不锁定显存频率的超频配置**（见下文"显存频率"）：Pearl 的超频会让 CMFD 慢约 6 倍。

**Windows：** `start-cmfd.bat` —— 填写 `WALLET`，保留 `POOL`（Aria）或去掉 Midas（或其他）那一行前的 `rem`，
可选 `DEVICES`、`MODE`、频率锁定。若显卡设置了降压偏移，请以管理员身份运行。

**参数：** `--cmfd-mode speed|eco`（默认 speed；eco：RTX 30 约省 13 % 功耗、算力降 2 %，RTX 40/50 两者相同），
`--cmfd-batch N`（1–64，默认按显存：11 GB 以上 64，否则 32），`--cmfd-model DIR`，`--cmfd-dir DIR`（引擎文件夹），
`--per-gpu-workers`（矿池上为每卡 `WORKER.gpuN`，而非整机一个 worker），`--lock-cclock/--lock-mclock/--moffset`
同第 7 节，`--devices`，`--minutes`。

**频率与降压。** CMFD 对张量核心的负载远高于 Pearl。对 Pearl 没问题的核心 V/F **偏移**（RTX 3070 +200，RTX 4070 Ti +330）
会让 CMFD 立即出错（"illegal memory access"、"misaligned address"）或给出错误结果。因此在 CMFD 模式下，矿工会在引擎启动前
**把正的核心偏移设为 0**，退出时恢复原值（日志：`cmfd offset: GPU0: core offset +345 -> 0 MHz ...`；异常退出后，下次启动会先恢复）。
这需要 root/管理员权限；没有权限时日志会提示，需要您自己去掉偏移（HiveOS：CMFD 飞行表的超频配置）。
`--uv-offset N` 则设置为 N —— **风险自负**。在 **AMD Radeon** 上 `--auto-uv` 通过 amduv 为 CMFD 调校降压（第 14 节，自 1.7.1 起）；NVIDIA 见下文。安全的做法：默认频率、功耗限制或锁定核心频率（`--lock-cclock`）。

**CMFD 的 `--auto-uv`（NVIDIA）。** 使用 `--auto-uv`（HiveOS：*Extra config arguments*；Windows：`start-cmfd.bat` 中
`AUTO_UV=1`，以管理员身份运行）时，矿工会**专门为 CMFD** 寻找降压偏移，与 Pearl 的结果分开保存。模型准备好之后、引擎启动之前，
对每张卡：引擎自带的 GPU 工作进程（`cmfd-v4-replay`，未修改）只加载一次模型，反复运行两批固定输入；在基准偏移（0，默认）下
记录其最终摘要作为参考，并运行 Common Foundry 1.0.16 官方测试向量；然后核心偏移以 `--uv-step`（15 MHz）为步长，从 `--uv-min`
（默认：0 之上一步）升到 `--uv-max`（默认：第 12 节的每卡上限：495，RTX 20xx / CMP 50HX 225，CMP 30HX/40HX 150）；每一步
运行 30 秒 + 10 秒测量，**每个摘要都必须与参考逐位一致**。第一个失败的步长即为边界；结果取低一步（`--uv-keep-last` 保留最后
成功的一步），再做 2 分钟确认 + 测试向量（失败则再降一步）。结果保存在 `uv.json`（`cmfd` 部分，按显卡 UUID，含驱动与引擎/工作进程
标识），之后的启动直接应用（若上次验证早于 `--uv-reval-hours`，先做 30 秒检查）。挖矿期间，已调优显卡上出现引擎
`INTEGRITY | ERROR` 或 CUDA 故障时，偏移退一步（并保存）并重启引擎。挖矿期间超频重新应用（HiveOS `nvidia-oc` 在矿工启动和
修改超频时写入配置的 *Core Clock* 值，字段为空时为 0）重置了已调优的偏移时，会将其恢复（自 1.7.1 起）：该卡的引擎停止，保存的
偏移在模型上做 30 秒快速检查 + 测试向量（失败则退一步），然后引擎以该偏移重启；每小时最多 4 次，其他值则调整器在该卡上退让。
检查期间被超频重置偏移的检查会重做。退出时恢复您原来的偏移（崩溃后在下次启动时恢复）。
若无法检查（没有模型、缺少工作进程、基准偏移下失败），该卡以 0 偏移挖矿并给出 WARNING。日志：`grep "auto-uv (cmfd)"`。
实测（Aria，统计行平均）：功耗限制 135 W 的 RTX 3070 在约 1050 MHz 处受功耗限制，电压已在曲线底部：边界 +225，结果 +195，
没有收益（135 W 下 13.84 → 13.97 FW/s）。同一张卡在 220 W：默认 19.5 FW/s、214 W（328 k FW/kWh）→ +135（边界 +165）
20.4–20.5 FW/s、213–216 W（342–345 k，**+4–5 %**）。CMP 50HX（功耗限制 225 W，核心锁定 1410 MHz）：到 +225 上限都未出错；
13.10 FW/s、216 W → 13.21、196 W（**+11 %**），13.01、217 W → 13.17、203 W（**+8 %**）。**边界取决于工作点**：功耗限制变化
超过 3 % 或 `--lock-cclock` 改变时会重新调优；在超频配置（HiveOS、nvidia-smi）中更改核心频率锁定后，请用 `--uv-reset` 运行一次。

**显存频率：CMFD 不要锁定显存。** Pearl 几乎不需要显存带宽，CMFD 则需要。常见的 Pearl 超频把显存频率锁得很低（HiveOS 超频
*Mem lock* 810、`nvidia-smi -lmc 810`、`--lock-mclock 810`），会让 CMFD 慢约 **6 倍**：在一台 4× RTX 3070 的 HiveOS 矿机上，
显存锁定在 810 MHz 时单卡 **3.7 FW/s**，不锁显存时 **21.8 FW/s**（约 237 W，功耗限制 240 W）。请为 CMFD 使用**单独的超频配置**：
不锁显存，核心偏移 0，功耗限制随意。自 1.6.1 起，矿工在每块 GPU 挖矿 30 秒后检查一次并给出警告
（`[GPU0] WARNING: memory clock 810 MHz of 7001 MHz while mining CMFD ...`）；它不会自行修改您的超频。

**日志。** 引擎输出带 `[GPUn]` 前缀（`MINER STATS`、`INTEGRITY | ok`、`SHARE STALE` = 份额传输途中出了新区块，无害；
`SHARE REJECTED` = 结果错误，请降低频率）。FreeForGem 每 10 秒打印一行
`[ 290s] GPU0 37.04 (190 W, 701390 FW/kWh) | total 37.04 FW/s | ... | user sub 84 acc 80 rej 0 | stale 4 | height 25089`
（抽水期间显示 `[DEVFEE]`；`STARTING` 表示引擎正在加载模型；另有 `RESTARTING`、`DISABLED`）。
引擎退出后分别在 15/30/60/120/300 秒后重启；连续 5 次启动失败（每次不到 2 分钟）后不再重启该 GPU，原因写在日志中
（避免无休止重连导致 IP 被封）。

**CMFD 开发者抽水：1 %**，在引擎内部：每块 GPU 每小时 36 秒，随机时刻，在您选择的矿池上，worker 为 `ffm-fee`，日志行
`DEV FEE | ...`。CMFD 模式下不运行第 10 节的 Pearl 抽水（合计 1 %）。

**算力**（Aria 矿池，5–30 分钟运行，偏移 0，自动批量）：RTX 4070 Ti（功耗限制 190 W）36.8 FW/s @ 187 W；
RTX 3070（功耗限制 135 W）14.1 FW/s @ 135 W —— 与同卡上的 FreeForgeMiner 1.2.3 相同（36.8 / 14.2 FW/s）。
默认功耗限制下 RTX 3070 约 20 FW/s（HiveOS，功耗限制 240 W，显存不锁定：21.8 FW/s @ 约 237 W）。

**故障排除：** `out of memory` —— 关闭占用 GPU 的程序，CMFD 需约 7 GB 空闲显存；`illegal memory access` /
`misaligned address` / `INTEGRITY | ERROR` —— 去掉核心偏移、降低频率；`model preparation ... failed` —— 检查能否访问
downloads.commonfoundry.ai 和 github.com 以及磁盘空间；矿池不接受地址 —— 从矿池网站获取当前地址（pin）。

### 14. AMD Radeon（Windows），1.6.0 新增

Windows 软件包也可以在 **AMD Radeon** 显卡上挖矿，使用自有的 OpenCL 内核（`pearlgpu_amd.dll`，与 CUDA 内核一样闭源）。
同一个程序、同样的启动文件、同样的日志；矿工自动选择 GPU 库：

* `--backend auto`（默认）：如果 CUDA 库能看到 NVIDIA 显卡就用 CUDA，否则用 AMD 库。`--backend amd` / `--backend cuda`
  （或 `FFGP_BACKEND`）可强制指定。日志中显示所选结果：`1 GPU(s) [amd]` 和 `[pearlgpu-amd] gfx1030: amd-a0 udot4 k8192`。
* 只使用**独立** Radeon 显卡；Ryzen 处理器的集成显卡 “AMD Radeon(TM) Graphics” 会被跳过。
* 一个进程只驱动一种厂商的显卡：同时有 NVIDIA 和 AMD 显卡的电脑请运行两个矿工（`--backend cuda` 和 `--backend amd`）。

| AMD GPU | 状态 |
|---|---|
| RX 6000（RDNA2，例如 RX 6800 XT） | **已测试**：Pearl 34.4 TH/s，约 260 W（默认设置），自检、矿池份额正常；CMFD 约 10 FW/s |
| RX 7000（RDNA3）、RX 9000（RDNA4） | 应可运行同一内核，**未测试** |
| RX 5000（RDNA1）、Vega 及更早、集成显卡 | 不支持 |
| Linux / HiveOS 上的 AMD | **暂不提供**（Linux 和 HiveOS 软件包仅支持 NVIDIA） |

**需要：** Windows 10/11 x64 和 AMD Adrenalin 驱动（自带 OpenCL）；不需要 NVIDIA 驱动。与 NVIDIA 一样运行任意 `start*.bat`。
自检：`freeforgem.exe --self-test`（将 AMD 内核与 CPU 参考实现比对，`== self-test PASSED ==`）。

**CPU 繁忙时：** 使用 AMD 时矿工主动等待 GPU，并以“高于正常”优先级运行，因此 CPU 矿工等占满所有核心的负载不会让 GPU 断供
（与占满所有核心的 CPU 矿工同时运行时 33.7–33.9 TH/s，而不是 32.6）。这会占用一个 CPU 线程；`PEARL_AMD_SPIN=0` / `FFGP_PROC_PRIO=0` 可关闭。

统计行中的**功耗**来自驱动（ADLX，芯片功耗，只读）。在多块 AMD 显卡的矿机上，单卡功耗可能在显卡之间对调（整机总和正确）。

**AMD 上的降压（`--auto-uv`）** 与 NVIDIA 不同：矿工启动同目录下的 `amduv.exe` 工具，它在最低核心电压下调出核心频率上限
（每个点都在 GPU 和 CPU 上逐位校验），每块卡 / 驱动 / AMD 库版本只做一次（约 10–15 分钟），之后每次启动直接应用结果。独立的守护进程
在矿工退出时（包括崩溃或被强制结束）**恢复您自己的频率 / 电压 / 功耗限制设置**。请以管理员身份运行。我们的 RX 6800 XT：约
30–32 TH/s @ 160–172 W，而默认为 34 TH/s @ ~260 W（TH/W 提高约 40 %）。`$FFGP_AMD_UV_GOAL=max` 保留更多速度；`--uv-reset`
清除结果。`--uv-offset`、`--lock-cclock`、`--lock-mclock` 和 `--moffset` 仅适用于 NVIDIA（AMD 下矿工会拒绝）。

**AMD 上的 CMFD：** `start-cmfd.bat` / `--pool cmfd+tls://…` 用法相同；软件包运行自带的 OpenCL 工作进程
`cmfd\cmfd-v4-replay-ocl.exe` 和矿池客户端 `cmfd\cmfd-miner-ffgp.exe`（用 `cmfd/patches/` 中的小补丁重新编译的
FreeForgeMiner，无需 nvidia-smi）。需要约 7 GB 空闲显存。RX 6800 XT：约 10 FW/s @ ~275 W（RTX 3070 约 20 FW/s）：CMFD 在
NVIDIA 张量核心上效率高得多。

**AMD 上 CMFD 的降压（自 1.7.1 起）：**删除 `start-cmfd.bat` 中 `set "AUTO_UV=1"` 前面的 `rem`（以管理员身份运行），或在 CMFD
命令行中加入 `--auto-uv`。`amduv.exe` 为 CMFD 保存**单独的结果**：在 Pearl 仍正常的频率下 CMFD 就会算错（RX 6800 XT 最低电压下：
Pearl 到 2250 MHz 都通过，CMFD 从 2200–2250 起出错），因此 CMFD 从不使用 Pearl 的点。每个点都在真实模型上检查：OpenCL 工作进程
反复计算固定的 32 个 nonce 批次，其最终摘要（覆盖全部 384 层的 BLAKE3）必须与默认设置下取得的摘要逐位一致——任何位置错一位都会改变摘要。
首次启动进行调校（约 15–20 分钟，每块卡 / 驱动 / 工作进程 / 模型只做一次），之后启动直接应用保存的点（上次检查超过 6 小时则做 15 秒检查）。
守护进程在矿工退出时（包括崩溃或被强制结束）**恢复您自己的频率 / 电压 / 功耗限制设置**。`start-cmfd.bat` 中的 `UV_GOAL`
（`$FFGP_AMD_UV_GOAL`）选择调校出的配置之一：

| 配置 | 如何得到 | RX 6800 XT（芯片功耗，ADLX） |
|---|---|---|
| —（默认设置） | | 10.0 FW/s @ 280 W（36 FW/kJ） |
| `eff` | 最低电压，提高频率上限直到检查失败，退回一步 | 2100 MHz / 881 mV：9.1 FW/s @ 193 W（47 FW/kJ，**+32 %**；FW/s −9 %） |
| `bal`（默认） | 在保留 ≥ 90 % 默认 FW/s 的前提下每瓦 FW 最高 | 在这块卡上 = `eff` |
| `max` | 您的最高频率，降低电压直到检查失败，再升回一步 | 2324 MHz / 1075 mV：10.1 FW/s @ 248 W（41 FW/kJ，+14 %；速度不变） |

通过矿工在 Aria 矿池上（各 10–15 分钟，统计行平均值）：默认 9.9 FW/s @ 277 W，`bal` 9.1 FW/s @ 191 W（FW/W +33 %），
`max` 10.0 FW/s @ 247 W（+14 %）；没有被拒份额，引擎 `INTEGRITY | ok`。边界每次运行略有浮动（881 mV 下 2200 MHz 两次通过、一次失败），
因此调校器总是留出一步余量。

在 Windows 上无用：功耗限制最多只能降到 −6 %（264 W，FW/W +4 %）；显存频率只能提高（2000–2150 MHz：没有提升，且 2150 时结果错误），
快速显存时序没有变化。`--uv-reset` 清除 Pearl 和 CMFD 的结果（下次 `--auto-uv` 启动重新调校）。

### 15. NOID（Parano1d），1.7.0 新增

FreeForGem 使用自有内核在 NVIDIA 显卡上挖 **NOID**（Parano1d：GF(2^128) 上的 Poseidon2b 工作量证明）（闭源，
`libnoidgpu.so` / `noidgpu.dll`，与 Pearl 库分开）。开发者抽水为 **1%**（每 100 分钟一个 60 秒时段，在您的矿池上），与 Pearl
相同。GPU 报告的每个候选结果在提交前都会用官方 `noid_poseidon2b` crate 在 CPU 上重新计算。

**系统要求**

* NVIDIA **RTX 30 / RTX 40 / RTX 50** 或 A100/H100（计算能力 8.0+）。RTX 20 / GTX 16 / CMP（Turing）和 AMD **不**支持 NOID。
* NVIDIA 驱动 **580 或更新**（内核使用 CUDA 13 的 `clmad` 指令）。驱动过旧时矿工会以
  `NOID needs an NVIDIA driver R580 or newer` 停止。HiveOS：`nvidia-driver-update 580`（或更新）。
* NOID 钱包地址（`o1...`）和 NOID 矿池。NOID 矿池使用基于 **TLS** 的 `parano1d-stratum-v1`：

| 矿池 | URL | 说明 |
|---|---|---|
| InnovLab（欧洲） | `stratum+ssl://eu2.innovlab.cc:19601` | 已用 FreeForGem 测试（份额被接受） |
| Suprnova | `stratum+ssl://noid.suprnova.cc:3340` | 已检查 TLS 握手，未实际挖矿 |

单独的 `HOST:PORT` 按 TLS 处理；`stratum+tcp://` 为普通 TCP；URL 末尾加 `?insecure` 可接受矿池的自签名证书。币种由
`--algo noid` 选择（矿池主机名含 `noid`、`parano1d` 或 `innovlab` 时自动选择）。矿池登录名为 `钱包.矿工名`。

**两个内核：`--noid-mode eff`（默认）和 `--noid-mode speed`**

RTX 3070，显存 810 MHz，功耗上限 135 W，每个点 60 秒（`eff` = 每个 SM 512 线程，`speed` = 更多 S-box 运算放在 CLMAD 上；
两者都使用共享操作数的 S-box 链 x·(x·x²)²）：

| 核心频率 / 偏移 | eff：MH/s @ W（MH/W） | speed：MH/s @ W（MH/W） |
|---|---|---|
| 1400 MHz / +220 | **34.70 @ 73.2（0.474）** | — |
| 1560 MHz / +205 | 38.40 @ 92.2（0.417）（偏移 +175） | 43.65 @ 105.3（0.415） |
| 1650 MHz / +220 | — | **46.16 @ 121.1（0.381）** |

* **eff**（默认，每瓦 MH 最高）：核心锁定 **1400 MHz** 并使用 `--auto-uv`。
* **speed**（MH/s 最高）：核心 **1560–1650 MHz**（RTX 30）：1560 MHz 时 43.65 MH/s，1650 MHz 时 46.16 MH/s。
* RTX 40/50：同样的内核分别为 sm_89 / sm_120 编译；频率请自行调整（从默认频率加 `--auto-uv` 开始）。

**命令行**

```
./freeforgem --algo noid --pool stratum+ssl://eu2.innovlab.cc:19601 --wallet o1您的地址 --worker rig1 \
             --noid-mode eff --lock-cclock 1400 --lock-mclock 810 --auto-uv
./freeforgem --algo noid --self-test          # 官方测试向量 + GPU 与 CPU 摘要比对，两个内核，所有 GPU
```

**HiveOS 飞行表：** Coin `NOID`（任意名称），Wallet `o1...`，Pool：Configure in miner；Miner **Custom** → Miner name
`freeforgem`，发布版的 Installation URL，Hash algorithm `parano1d`，Wallet and worker template `%WAL%.%WORKER_NAME%`，
Pool URL `stratum+ssl://eu2.innovlab.cc:19601`，Extra config `--algo noid --lock-cclock 1400 --auto-uv`。面板中的算力单位
为 kH/s（MH/s × 1000）。

**Windows：** 编辑 `start-noid.bat`（WALLET、POOL、MODE，可选 AUTO_UV=1 和 LOCK_CCLOCK；使用 AUTO_UV/频率设置时以管理员
身份运行），然后双击运行。

**NOID 的降压。** NOID 需要自己的偏移：Pearl 的降压对它通常太深（RTX 3070：Pearl 的 +250 会让 NOID 结果出错；
RTX 4070 Ti：机主为 Pearl 设置的 +300 会让 NOID 崩溃）。因此：

* 不使用 `--auto-uv` 时，NOID 运行期间**正的核心偏移会被设为 0**，退出时恢复；
* 使用 `--auto-uv` 时，FreeForGem 为每张卡**和每种模式**单独调整 NOID 偏移（每步为何需要 40 秒见第 16 节）：先检查基础偏移，
  然后从 `+120`（`--uv-min`）开始以 15 MHz（`--uv-step`）步进直到该卡的上限（`--uv-max`，各卡默认值与 Pearl 相同）；第一个
  失败的步骤即为边缘，结果取最后一个成功步骤再低一步，并用 2 分钟运行确认。首次启动约 10 分钟；之后的启动直接应用已保存
  的结果（若早于 `--uv-reval-hours` 则先检查 30 秒）。结果保存在 `uv.json` 的 `noid` 部分（Pearl 与 CMFD 的结果保持不变），
  并记录驱动、内核、核心锁频和功耗上限：任何一项改变都会重新调整；
* `--uv-offset N` 直接设置 N，不做检查（风险自负）；`--uv-reset` 清除 NOID 结果；
* 挖矿时每个候选结果都在 CPU 上复核，每 120 秒每张 GPU 还会运行一次简短的看门狗检查（固定任务 6 次启动，逐位比对）；
  检查失败会让该卡的偏移回退一步（并保存），挖矿继续；
* 挖矿期间的超频重新应用（HiveOS `nvidia-oc` 在矿工启动和修改超频时写入配置的 *Core Clock* 值，字段为空时为 0；Pearl 的偏移
  会让 NOID 静默算错）：每 30 秒读回偏移，已调优的 NOID 偏移在该卡暂停挖矿、做 30 秒压力检查后恢复（失败则退一步），基准 0
  （未用 `--auto-uv`）或 `--uv-offset` 的值立即重新设置；每小时最多 4 次，其他值则矿工在该卡上退让（自 1.7.1 起）。检查期间被
  超频重置偏移的检查会重做；
* RTX 3070 实测：**eff @1400 MHz** — 偏移 0 时 87.5 W → **+195 时 72.8 W**（34.3 MH/s；电压下限：高于 +195 功耗不再
  下降，调整器在此停止），MH/W **+20%**；矿池上 34.8 MH/s @ 73 W（0.476 MH/W），41/41 份额被接受。**speed @1560 MHz** —
  127.8 W → **+195 时 106.2 W**（43.3 MH/s），边缘 +225，MH/W **+20%**；矿池上 43.8 MH/s @ 107 W（0.410），24/24 被接受。
  RTX 4070 Ti 默认频率、功耗上限 190 W、偏移 0：speed 82.6 MH/s，eff 约 81 MH/s（均受功耗上限限制；尚未调整）。

**日志（NOID）**

```
FreeForGem 1.7.2-beta | NOID (parano1d) | pool stratum+ssl://eu2.innovlab.cc:19601 | worker rig1 | 1 GPU(s) [0] | mode eff
[noid-pool:user] subscribed: parano1d-stratum-v1, nonce_bits 64, session namespace 24e67d0000000200
new job 46f6...0000 [user] height 239348 (share target 0000003fffffffff.., expires in 262s)
[  60s] GPU0 34.700 (73 W, 0.474 MH/W) | total 34.700 MH/s, 73 W, 0.474 MH/W | hits 412 (bad 0, stale 0) | user sub 3 acc 3 rej 0 | fee sub 0 acc 0 rej 0
```

`hits` = 在 CPU 上复核的候选结果（内核报告的数量多于份额，约每 400 万个 nonce 一个）；`bad` = 未通过检查的候选或看门狗启动
（GPU 结果错误：偏移会回退）；`stale` = 矿池已替换的任务的份额（不发送）。`pool pause (canonical-tip-unverified)` 属于正常
现象：矿池丢弃当前任务并立即发送下一个。

### 16. 降压的“静默边缘”（为什么 `--auto-uv` 要检查这么多）

提高核心偏移时，GPU 在相同频率下获得更低的电压。离极限很远时一切正常。接近极限时，显卡**不会**崩溃，驱动**不**报告任何
错误（没有 Xid），快速自检也**能通过**——但部分结果已经**是错误的**。我们在 NOID 上的实测（RTX 3070，1560 MHz）：在 +250 时，
**5984 次**完整启动中有 **168 次**返回了错误的命中集合，第一次出现在负载约 12 秒后，期间没有任何驱动错误；2 分钟的自检照样
通过。低一步（+235）仍然出错；+220 完全正确。

后果：错误结果会变成**无效份额**或**永远找不到的份额**。这些在普通的矿工日志中都看不到——算力看起来一样，只是矿池付款变少。
只监视崩溃和 Xid 错误的矿工会直接把参数调进这个区域。

FreeForGem 的做法（Pearl、CMFD 和 NOID 遵循同一原则）：

1. **每一步都在满负载下多次逐位检查**——而不是只做一次自检。Pearl：固定任务的数千次尝试，每个 tile 的摘要与第一次运行比对，
   找到的结果用官方验证器在 CPU 上验证。NOID：长时间压力检查——每步 40 秒的整网格启动，每次启动的候选集合与官方 crate
   在 CPU 上算出的集合比对（RTX 3070 上每步约 4000 次启动）。CMFD：在真实的 6.4 GB 模型上将固定批次的摘要与默认设置下的
   参考值比对（1.7.2-beta 中 CMFD 以偏移 0 运行；CMFD 调整器单独发布）。
2. **第一个失败的步骤标记边缘。** 结果取**再低一步**（15 MHz 余量；`--uv-keep-last` 保留最后一个成功步骤），并用 2 分钟运行
   **确认**；确认失败则再降一步。
3. **挖矿期间持续检查结果：** 每个 Pearl 命中和每个 NOID 候选在提交前都在 CPU 上重新计算；NOID 还每 120 秒进行一次看门狗
   检查；Pearl 每 6 小时重新验证。检查失败时**自动将偏移回退一步**（并保存），挖矿继续。
4. 机主的偏移在第一次修改前写入恢复文件，并在**退出时恢复**（崩溃后则在下次启动时恢复）。

日志中的样子（speed 内核，RTX 3070，`--lock-cclock 1560 --auto-uv`，我们的实际运行；最后两行来自故意设置过深的 `--uv-offset 280` 的运行）：

```
auto-uv (noid): GPU0 (bus 0d, RTX 3070) tuning the NOID offset (speed kernel speed:aff/sm86): base +0, start +120, limit +495 (default), step 15, 40 s of stress per step
auto-uv (noid): GPU0 (bus 0d, RTX 3070) base +0: ok (3448 launches checked, 43.29 MH/s, 127.8 W, 0.339 MH/W, 1560 MHz, 56 C)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) +120: ok (4597 launches checked, 43.30 MH/s, 114.4 W, 0.378 MH/W, 1560 MHz, 56 C)
...
auto-uv (noid): GPU0 (bus 0d, RTX 3070) +210: ok (4598 launches checked, 43.30 MH/s, 103.8 W, 0.417 MH/W, 1560 MHz, 55 C)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) +225: FAILED (WRONG RESULTS: 5 of 4597 launches gave a wrong candidate set, first after 14.1s (no Xid, no crash: the silent edge)): edge found
auto-uv (noid): GPU0 (bus 0d, RTX 3070) confirmation +195: ok (13790 launches checked, 43.30 MH/s, 106.2 W, 0.408 MH/W, 1560 MHz, 57 C)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) RESULT speed: offset +195 (edge +225, one step below the last good one); MH/W 0.339 -> 0.408 (+20.3%); saved to .../uv.json
...
GPU0: WATCHDOG: 1 of 6 check launches gave WRONG results (silent undervolt edge?)
auto-uv (noid): GPU0: wrong GPU results at offset +280 (CPU check of a hit / watchdog): stepped back to +265 and mining on
```

### 17. 按锁频保存的配置、调优期间继续挖矿、挖矿中的崩溃（1.7.2 新增）

**原因。** 降压边缘随核心频率移动：在 1410 MHz 验证的偏移在 1805 MHz 下并不安全。在一台 8x RTX 3070 的 HiveOS 矿机上，另一个挖矿
程序留下了 1400 MHz 锁频，auto-uv 在此找到 +450..+495；HiveOS 的 1805 MHz 锁频恢复后，这些偏移被"不经启动检查直接应用"，导致
illegal memory access、Xid 13/43 与重启循环。1.7.2 的改动：

**每个锁频一个配置。** auto-uv 按显卡、币种 / NOID 模式与核心锁频分别保存结果（`uv.json` 的 `profiles`、`noid`、`cmfd_profiles`；
矿工设置显存锁频时其也属于键，如 `1805/mem810`）：

* 使用 `--lock-cclock N` 时锁频已知并选择对应配置；1805 -> 1410 -> 1805 切换时立即复用各锁频已验证的结果（`--uv-reval-hours`
  内验证过则直接应用，否则先做常规快速检查）；没有配置的锁频进行调优，其他锁频的配置保持不变；
* 不使用 `--lock-cclock` 时矿工无法读取锁频：先以您的偏移开始挖矿，用约 20 秒的挖矿负载频率选择配置（`1805`、`~1815` 或
  `unlocked`）；没有合适的配置则针对当前频率调优。NOID 与 CMFD 在基础偏移上做短检查来测频率；
* 启动日志列出每张卡的配置：`profiles: 1400:+210, 1900:+180; using 1400 ...`；
* `--uv-reset` 只清除本次运行频率（及币种 / 模式）的配置；`--uv-reset-all` 清除全部；
* 1.7.2 之前的结果（每卡一个，未记录锁频）成为其负载频率（`~1410`）的配置，仅在当前频率下检查通过后使用；NOID 旧结果仅保留
  使用 `--lock-cclock` 调优的；
* 挖矿中同一偏移下负载频率上升（锁频被提高）时恢复您的偏移并重新按频率选择配置。

**调优期间继续挖矿。** 1.7.2 之前所有显卡都要等最慢的调优结束才开始挖矿。现在挖矿立即开始；每张卡的检查与调优在后台进行，只在检查
期间暂停该卡（其调优或快速检查期间），其他卡继续挖矿。

**您的偏移。** 在 HiveOS 上，退出时恢复的是该卡 OC 配置的 *Core Clock* 值（`/hive-config/nvidia-oc.conf`），而不是启动时卡上残留
的值。在当前频率下验证过且低于您的偏移的结果会**代替您的偏移使用**，绝不在未检查的情况下保留。若调优的起始偏移本身失败，会以
60 MHz 步进寻找能通过的更低偏移并保存为配置。

**哪张卡出错。** 一张卡上的 CUDA 故障会结束**整个矿工进程**的 CUDA。矿工现在：通过驱动 Xid 事件确定出错的卡并记录在
`gpu-faults.json`；自行重启（Linux / HiveOS 同一进程内，每小时最多 12 次）；使用 `--auto-uv` 时崩溃的降压偏移视为失败并在其下方
重新调优，在您自己的偏移上连续 2 次启动崩溃则降低它（先降到 HiveOS 配置的 Core Clock 值，再以 60 MHz 步进降到 0）；无可再降仍
崩溃的卡暂停挖矿一小时，其他卡继续。无故障 30 分钟清零计数；`--uv-reset` 不清除 `gpu-faults.json`（删除该文件即可）。
