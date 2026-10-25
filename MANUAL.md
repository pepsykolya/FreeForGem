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
https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.0-beta/freeforgem-1.7.0beta.tar.gz
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
tar xzf freeforgem-1.7.0-beta-linux-x86_64.tar.gz
cd freeforgem-1.7.0-beta
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

1. Download **`freeforgem-1.7.0-beta-windows-x64.zip`** from the release page:
   `https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.0-beta/freeforgem-1.7.0-beta-windows-x64.zip`
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
cd /d C:\Mining\freeforgem-1.7.0-beta-windows-x64
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

* HiveOS: Overclock tab → core clock (absolute), memory clock (absolute), power limit; the V/F offset via `--auto-uv`.
* Linux: `nvidia-smi -lgc 1560,1560`, `nvidia-smi -lmc 810,810`, `nvidia-smi -pl 120` (root), or `--lock-cclock` / `--lock-mclock`.
* Windows: `LOCK_CCLOCK` / `LOCK_MCLOCK` / `MOFFSET` in `start.bat` (as Administrator), `start-nvidia-smi.bat` for a power
  limit, or MSI Afterburner. Ready combinations per card: section 12, *Windows: ready combinations*.

If shares get rejected or the log shows `bad` hits, lower the core clock or the offset.

### 8. Reading the log

| Line | Meaning |
|---|---|
| `FreeForGem 1.7.0-beta \| pool ... \| worker ... \| N GPU(s)` | start banner |
| `[pearlgpu] search kernel: SASS-patched cubin (...)` | the tuned kernel is loaded. `embedded search kernel could not be loaded (reason)` = the miner stops with an error (driver older than 525 or an unsupported GPU) |
| `new job ... prepared in ...` | a new pool job is ready |
| `[ 120s] GPU0 ... \| total X TH/s \| hits ... \| user sub S acc A rej R \| fee sub ... acc ... rej ...` | every 10 s: hashrate per GPU and in total; shares submitted / accepted / rejected for you and for the dev fee; `[DEVFEE]` while a fee slot runs Since 1.5.3 each GPU also shows its board power and efficiency, e.g. `GPU0 49.906 (151 W, 0.330 TH/W)`, and the total `\| 151 W, 0.330 TH/W` (when NVML is available) |
| `[DISCONNECTED]` | the pool connection is down; the miner reconnects by itself |
| `devfee: ...` | the dev fee slots (section 10) |
| `!! ...` | a warning or error (a GPU worker stopped, a hit was not submitted) |

### 9. Verifying the download (SHA-256)

Compare the result with the hash on the release page.

* Linux: `sha256sum freeforgem-1.7.0-beta-linux-x86_64.tar.gz freeforgem-1.7.0beta.tar.gz`
* Windows: `certutil -hashfile freeforgem-1.7.0-beta-windows-x64.zip SHA256` or
  `Get-FileHash freeforgem-1.7.0-beta-windows-x64.zip -Algorithm SHA256`

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
5. While mining: every 6 hours (`--uv-reval-hours`) the setting is re-checked (mining on that GPU pauses for about 30 s).
   If a found result fails the CPU verification, or a burst of rejected shares appears, the offset goes one step down (never
   below your own value) and is saved. If somebody else changes the offset (for example HiveOS re-applies its OC), auto-uv
   stands down on that GPU and does not fight.
6. On exit (Ctrl+C, SIGTERM, HiveOS `miner stop`, closing the Windows console, normal end) your offset is restored on every
   GPU (before 1.6.1 a `miner stop` on HiveOS could restore only GPU0). If the miner was killed (`kill -9`, crash), the next start
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
     `set "UV_MAX="` (empty = per card: +495, RTX 20xx/GTX 16xx/CMP 50HX +225, CMP 30HX/40HX +150; RTX 40/50 often hold +450..480, RTX 30
     usually +200..360). A comma list sets one value per GPU in `DEVICES` order, e.g. `set "UV_MAX=225,495,150"`.
   * `start-nvidia-smi.bat`: `AUTO_UV=1`, the range in `UV_MIN` / `UV_MAX`, plus power limit and clock lock through `nvidia-smi`.
   * your own command line: `freeforgem.exe --pool ... --wallet ... --auto-uv --uv-max 480`.
4. **Starting point.** The tuner starts from the offset the card has now and never goes below it. The *Core Clock* offset of
   MSI Afterburner (or a similar tool) is this same offset, so a value you set there is your starting point and is put back
   on exit. Do not let Afterburner re-apply a profile while mining: auto-uv sees that somebody else changed the offset and
   stands down on that GPU. With no offset set the tuner starts at +120, on RTX 40 at +200, on RTX 50 at +300 (or `--uv-min` / `UV_MIN`).
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

**GPU clock control (optional, NVML; needs root / Administrator, works on Windows — new in 1.4.9).** `--lock-cclock N` locks the core clock to N MHz (like `nvidia-smi -lgc N,N`), `--lock-mclock N` locks the memory clock to N MHz (like `-lmc`), and `--moffset N` sets a memory-clock offset in MHz, which may be negative (needs driver 555+). Each takes one value for all GPUs, or a comma-separated list in `--devices` order (one value per device). The settings are applied at start and undone on exit (Ctrl-C / SIGTERM / crash). On Windows set `LOCK_CCLOCK`, `LOCK_MCLOCK`, `MOFFSET` in the `.bat` file. Example: `--lock-cclock 1500 --lock-mclock 810 --moffset -200`.

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
| Kavatar (kavatar.kasplay.online, 1 %) | `cmfd+tls://188.35.20.44:29445?pin=71ab1d2e36cd20226ac16184b91dda5527126fa8b29ee1de2b6f5cd8b7028041` |
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
the OC profile of the CMFD flight sheet). `--uv-offset N` sets N instead — **at your own risk**. `--auto-uv` is Pearl-only.
Safe settings: stock clocks, a power limit, or a core lock (`--lock-cclock`).

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

**Hashrate** (measured on the Aria and Kavatar pools, 5–30-minute runs, offset 0, auto batch): RTX 4070 Ti (power limit 190 W)
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

RTX 3070, memory 810 MHz, power limit 135 W, 60 s per point (`eff` = 512 threads per SM, `speed` = more of the S-box on CLMAD):

| Core clock / offset | eff: MH/s @ W (MH/W) | speed: MH/s @ W (MH/W) |
|---|---|---|
| 1400 MHz / +220 | **35.69 @ 76.4 (0.467)** | 38.68 @ 85.3 (0.454) |
| 1560 MHz / +205 | 39.30 @ 94.6 (0.415) (n2gc) | 42.79 @ 105.1 (0.407) |
| 1650 MHz / +220 | — | **45.25 @ 121.0 (0.374)** |

* **eff** (default, best MH per watt): lock the core at **1400 MHz** and use `--auto-uv`.
* **speed** (best MH/s): core **1560–1650 MHz** (RTX 30); at 1650 it is faster than the fastest closed miner we measured on
  the same card (42.55 MH/s at 1560).
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
  job, bit-exact) runs on each GPU; a failure steps that card's offset back by one step (saved) and mining goes on.
* RTX 3070 measured edges (agent stress check, 1400–1650 MHz): last good +220…+235, first failure +235…+250 → results
  +205…+220. Below ~1400 MHz the card sits at its minimum voltage and the offset no longer saves power.

**Log (NOID)**

```
FreeForGem 1.7.0-beta | NOID (parano1d) | pool stratum+ssl://eu2.innovlab.cc:19601 | worker rig1 | 1 GPU(s) [0] | mode eff
[noid-pool:user] subscribed: parano1d-stratum-v1, nonce_bits 64, session namespace 24e67d0000000200
new job 46f6...0000 [user] height 239348 (share target 0000003fffffffff.., expires in 262s)
[  60s] GPU0 35.690 (76 W, 0.467 MH/W) | total 35.690 MH/s, 76 W, 0.467 MH/W | hits 412 (bad 0, stale 0) | user sub 3 acc 3 rej 0 | fee sub 0 acc 0 rej 0
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
   6.4 GB model compared with the stock reference (CMFD runs at offset 0 in 1.7.0-beta; the CMFD tuner ships separately).
2. **The first failing step marks the edge.** The result is **one step lower** (15 MHz margin; `--uv-keep-last` keeps the last
   good step) and is **confirmed** by a 2-minute run; if the confirmation fails, one more step down.
3. **While mining, results keep being checked:** every Pearl hit and every NOID candidate is recomputed on the CPU before it
   is submitted; NOID also runs a watchdog check every 120 s; Pearl re-validates every 6 hours. A failed check **steps the
   offset back automatically** (saved) and mining goes on.
4. The owner's offset is written to a restore file before the first change and **put back on exit** (and by the next start
   after a crash).

What you see in the log (NOID, RTX 3070, `--lock-cclock 1560`, illustrative):

```
auto-uv (noid): GPU0 (bus 0d, RTX 3070) base 0: ok (3020 launches checked, 37.10 MH/s, 112.4 W, 0.330 MH/W, 1560 MHz, 61 C)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) +205: ok (4012 launches checked, 37.57 MH/s, 109.4 W, 0.343 MH/W, 1560 MHz, 63 C)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) +220: ok (4010 launches checked, ...)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) +235: FAILED (WRONG RESULTS: 3 of 1046 launches gave a wrong candidate set, first after 9.8s (no Xid, no crash: the silent edge)): edge found
auto-uv (noid): GPU0 (bus 0d, RTX 3070) confirmation +205: ok (...)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) RESULT eff: offset +205 (edge +235, one step below the last good one); MH/W 0.330 -> 0.343 (+3.9%)
...
GPU0: WATCHDOG: 1 of 6 check launches gave WRONG results (silent undervolt edge?)
auto-uv (noid): GPU0: wrong GPU results at offset +205 (CPU check of a hit / watchdog): stepped back to +190 and mining on
```

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
https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.0-beta/freeforgem-1.7.0beta.tar.gz
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
tar xzf freeforgem-1.7.0-beta-linux-x86_64.tar.gz
cd freeforgem-1.7.0-beta
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

1. Скачайте **`freeforgem-1.7.0-beta-windows-x64.zip`** со страницы релиза:
   `https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.0-beta/freeforgem-1.7.0-beta-windows-x64.zip`
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
cd /d C:\Mining\freeforgem-1.7.0-beta-windows-x64
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

* HiveOS: вкладка Overclock → частота ядра (абсолютная), частота памяти (абсолютная), лимит мощности; V/F offset — через `--auto-uv`.
* Linux: `nvidia-smi -lgc 1560,1560`, `nvidia-smi -lmc 810,810`, `nvidia-smi -pl 120` (root) или `--lock-cclock` / `--lock-mclock`.
* Windows: `LOCK_CCLOCK` / `LOCK_MCLOCK` / `MOFFSET` в `start.bat` (от администратора), `start-nvidia-smi.bat` — если нужен
  лимит мощности, или MSI Afterburner. Готовые комбинации по картам — раздел 12, *Windows: готовые комбинации*.

Если пул отклоняет шары или в логе есть `bad`-попадания, снизьте частоту ядра или offset.

### 8. Как читать лог

| Строка | Значение |
|---|---|
| `FreeForGem 1.7.0-beta \| pool ... \| worker ... \| N GPU(s)` | стартовая строка |
| `[pearlgpu] search kernel: SASS-patched cubin (...)` | загружено настроенное ядро. `embedded search kernel could not be loaded (причина)` = майнер останавливается с ошибкой (драйвер старше 525 или неподдерживаемая карта) |
| `new job ... prepared in ...` | готово новое задание пула |
| `[ 120s] GPU0 ... \| total X TH/s \| hits ... \| user sub S acc A rej R \| fee sub ... acc ... rej ...` | каждые 10 с: хэшрейт по картам и общий; отправленные / принятые / отклонённые шары ваши и комиссии; `[DEVFEE]` пока идёт слот комиссии С 1.5.3 у каждой карты также видны потребление и эффективность, например `GPU0 49.906 (151 W, 0.330 TH/W)`, и итог `\| 151 W, 0.330 TH/W` (если доступен NVML) |
| `[DISCONNECTED]` | соединение с пулом потеряно; майнер переподключится сам |
| `devfee: ...` | слоты комиссии (раздел 10) |
| `!! ...` | предупреждение или ошибка (остановился GPU-воркер, hit не отправлен) |

### 9. Проверка загрузки (SHA-256)

Сравните результат с хешем на странице релиза.

* Linux: `sha256sum freeforgem-1.7.0-beta-linux-x86_64.tar.gz freeforgem-1.7.0beta.tar.gz`
* Windows: `certutil -hashfile freeforgem-1.7.0-beta-windows-x64.zip SHA256` или
  `Get-FileHash freeforgem-1.7.0-beta-windows-x64.zip -Algorithm SHA256`

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
5. Во время майнинга: каждые 6 часов (`--uv-reval-hours`) настройка перепроверяется (майнинг на этой карте приостанавливается
   примерно на 30 с). Если найденный результат не прошёл проверку на CPU или пошла серия отклонённых шар, offset
   снижается на шаг (но не ниже вашего значения) и сохраняется. Если offset изменил кто-то другой (например, HiveOS заново
   применил разгон), auto-uv отступает на этой карте и не воюет.
6. При выходе (Ctrl+C, SIGTERM, `miner stop` в HiveOS, закрытие окна консоли в Windows, обычное завершение) ваш offset
   возвращается на всех GPU (до 1.6.1 `miner stop` в HiveOS мог вернуть только GPU0). Если майнер был убит (`kill -9`, сбой), то при
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
     `set "UV_MAX="` (пусто = по карте: +495, RTX 20xx/GTX 16xx/CMP 50HX +225, CMP 30HX/40HX +150; RTX 40/50 часто держат +450..480, RTX 30
     обычно +200..360). Список через запятую задаёт значение для каждой карты в порядке `DEVICES`, например `set "UV_MAX=225,495,150"`.
   * `start-nvidia-smi.bat`: `AUTO_UV=1`, диапазон в `UV_MIN` / `UV_MAX`, плюс лимит мощности и фиксация частот через `nvidia-smi`.
   * своя командная строка: `freeforgem.exe --pool ... --wallet ... --auto-uv --uv-max 480`.
4. **Точка старта.** Подбор начинает с offset, который сейчас стоит на карте, и ниже него не опускается. Смещение *Core Clock*
   в MSI Afterburner (и похожих программах) — это тот же самый offset: выставленное там значение станет точкой старта и
   вернётся при выходе. Не давайте Afterburner заново применять профиль во время майнинга: auto-uv увидит, что offset
   поменял кто-то другой, и отступит на этой карте. Если offset не задан, подбор стартует с +120, на RTX 40 — с +200, на RTX 50 — с +300 (или с `--uv-min` / `UV_MIN`).
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

**Управление частотами GPU (по желанию, через NVML; нужен root / администратор, работает в Windows — с 1.4.9).** `--lock-cclock N` фиксирует частоту ядра на N МГц (как `nvidia-smi -lgc N,N`), `--lock-mclock N` фиксирует частоту памяти на N МГц (как `-lmc`), `--moffset N` задаёт смещение частоты памяти в МГц, может быть отрицательным (нужен драйвер 555+). Каждая опция принимает одно значение для всех GPU или список через запятую в порядке `--devices` (по одному на карту). Настройки применяются при старте и снимаются при выходе (Ctrl-C / SIGTERM / сбой). В Windows задайте `LOCK_CCLOCK`, `LOCK_MCLOCK`, `MOFFSET` в `.bat`-файле. Пример: `--lock-cclock 1500 --lock-mclock 810 --moffset -200`.

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
| Kavatar (kavatar.kasplay.online, 1 %) | `cmfd+tls://188.35.20.44:29445?pin=71ab1d2e36cd20226ac16184b91dda5527126fa8b29ee1de2b6f5cd8b7028041` |
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
`--auto-uv` только для Pearl. Безопасно: стоковые частоты, лимит мощности или лок ядра (`--lock-cclock`).

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

**Хешрейт** (пулы Aria и Kavatar, прогоны 5–30 минут, смещение 0, батч авто): RTX 4070 Ti (лимит 190 Вт) 36,8 FW/s при 187 Вт;
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

RTX 3070, память 810 МГц, лимит мощности 135 Вт, по 60 с на точку (`eff` — 512 потоков на SM, `speed` — больше S-box на CLMAD):

| Частота ядра / offset | eff: MH/s @ Вт (MH/Вт) | speed: MH/s @ Вт (MH/Вт) |
|---|---|---|
| 1400 МГц / +220 | **35.69 @ 76.4 (0.467)** | 38.68 @ 85.3 (0.454) |
| 1560 МГц / +205 | 39.30 @ 94.6 (0.415) (n2gc) | 42.79 @ 105.1 (0.407) |
| 1650 МГц / +220 | — | **45.25 @ 121.0 (0.374)** |

* **eff** (по умолчанию, лучшие MH на ватт): лок ядра **1400 МГц** и `--auto-uv`.
* **speed** (максимум MH/s): ядро **1560–1650 МГц** (RTX 30); на 1650 быстрее самого быстрого закрытого майнера, которого мы
  мерили на той же карте (42.55 MH/s на 1560).
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
* края на RTX 3070 (stress-проверка, 1400–1650 МГц): последний хороший +220…+235, первая ошибка +235…+250 → результаты
  +205…+220. Ниже ~1400 МГц карта уже на минимальном напряжении, и offset больше не экономит ватты.

**Лог (NOID)**

```
FreeForGem 1.7.0-beta | NOID (parano1d) | pool stratum+ssl://eu2.innovlab.cc:19601 | worker rig1 | 1 GPU(s) [0] | mode eff
[noid-pool:user] subscribed: parano1d-stratum-v1, nonce_bits 64, session namespace 24e67d0000000200
new job 46f6...0000 [user] height 239348 (share target 0000003fffffffff.., expires in 262s)
[  60s] GPU0 35.690 (76 W, 0.467 MH/W) | total 35.690 MH/s, 76 W, 0.467 MH/W | hits 412 (bad 0, stale 0) | user sub 3 acc 3 rej 0 | fee sub 0 acc 0 rej 0
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
   фиксированных батчей на настоящей модели 6.4 ГБ сравниваются с эталоном на стоке (в 1.7.0-beta CMFD работает с offset 0;
   тюнер CMFD выходит отдельно).
2. **Первый шаг с ошибкой — это край.** Результат берётся **на шаг ниже** (запас 15 МГц; `--uv-keep-last` оставляет последний
   хороший шаг) и **подтверждается** 2-минутным прогоном; если подтверждение не прошло — ещё на шаг ниже.
3. **В майнинге результаты продолжают проверяться:** каждый хит Pearl и каждый кандидат NOID пересчитывается на CPU перед
   отправкой; у NOID ещё сторожевая проверка каждые 120 с; Pearl перепроверяется каждые 6 часов. Неудачная проверка
   **автоматически откатывает offset на шаг** (с сохранением), майнинг продолжается.
4. Offset владельца записывается в файл восстановления до первого изменения и **возвращается при выходе** (а после падения — при
   следующем запуске).

Как это выглядит в логе (NOID, RTX 3070, `--lock-cclock 1560`, пример):

```
auto-uv (noid): GPU0 (bus 0d, RTX 3070) base 0: ok (3020 launches checked, 37.10 MH/s, 112.4 W, 0.330 MH/W, 1560 MHz, 61 C)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) +205: ok (4012 launches checked, 37.57 MH/s, 109.4 W, 0.343 MH/W, 1560 MHz, 63 C)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) +220: ok (4010 launches checked, ...)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) +235: FAILED (WRONG RESULTS: 3 of 1046 launches gave a wrong candidate set, first after 9.8s (no Xid, no crash: the silent edge)): edge found
auto-uv (noid): GPU0 (bus 0d, RTX 3070) confirmation +205: ok (...)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) RESULT eff: offset +205 (edge +235, one step below the last good one); MH/W 0.330 -> 0.343 (+3.9%)
...
GPU0: WATCHDOG: 1 of 6 check launches gave WRONG results (silent undervolt edge?)
auto-uv (noid): GPU0: wrong GPU results at offset +205 (CPU check of a hit / watchdog): stepped back to +190 and mining on
```

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
https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.0-beta/freeforgem-1.7.0beta.tar.gz
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
tar xzf freeforgem-1.7.0-beta-linux-x86_64.tar.gz
cd freeforgem-1.7.0-beta
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

1. 从发布页面下载 **`freeforgem-1.7.0-beta-windows-x64.zip`**：
   `https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.0-beta/freeforgem-1.7.0-beta-windows-x64.zip`
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
cd /d C:\Mining\freeforgem-1.7.0-beta-windows-x64
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

* HiveOS：Overclock 选项卡 → 核心频率（绝对值）、显存频率（绝对值）、功耗限制；V/F 偏移用 `--auto-uv`。
* Linux：`nvidia-smi -lgc 1560,1560`、`nvidia-smi -lmc 810,810`、`nvidia-smi -pl 120`（root），或 `--lock-cclock` / `--lock-mclock`。
* Windows：在 `start.bat` 中设置 `LOCK_CCLOCK` / `LOCK_MCLOCK` / `MOFFSET`（以管理员身份），需要功耗限制时用 `start-nvidia-smi.bat`，
  或 MSI Afterburner。各显卡的现成组合见第 12 节 *Windows 现成组合*。

如果份额被拒绝或日志出现 `bad` 命中，请降低核心频率或偏移。

### 8. 如何阅读日志

| 行 | 含义 |
|---|---|
| `FreeForGem 1.7.0-beta \| pool ... \| worker ... \| N GPU(s)` | 启动横幅 |
| `[pearlgpu] search kernel: SASS-patched cubin (...)` | 已加载调优内核。`embedded search kernel could not be loaded (原因)` = 矿工报错退出（驱动低于 525 或显卡不受支持） |
| `new job ... prepared in ...` | 新的矿池任务已就绪 |
| `[ 120s] GPU0 ... \| total X TH/s \| hits ... \| user sub S acc A rej R \| fee sub ... acc ... rej ...` | 每 10 秒：每块显卡和总算力；你和抽水的已提交 / 已接受 / 已拒绝份额；抽水时段显示 `[DEVFEE]` 自 1.5.3 起每块显卡还显示功耗和能效，例如 `GPU0 49.906 (151 W, 0.330 TH/W)`，以及总计 `\| 151 W, 0.330 TH/W`（需要 NVML） |
| `[DISCONNECTED]` | 与矿池断开；矿工会自动重连 |
| `devfee: ...` | 抽水时段（第 10 节） |
| `!! ...` | 警告或错误（GPU 工作线程停止、某个 hit 未提交） |

### 9. 校验下载（SHA-256）

将结果与发布页面上的哈希比对。

* Linux：`sha256sum freeforgem-1.7.0-beta-linux-x86_64.tar.gz freeforgem-1.7.0beta.tar.gz`
* Windows：`certutil -hashfile freeforgem-1.7.0-beta-windows-x64.zip SHA256` 或
  `Get-FileHash freeforgem-1.7.0-beta-windows-x64.zip -Algorithm SHA256`

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
5. 挖矿期间：每 6 小时（`--uv-reval-hours`）重新检查一次（该显卡的挖矿暂停约 30 秒）。如果找到的结果未通过 CPU 验证，
   或出现一批被拒绝的份额，偏移下调一步（不会低于你自己的值）并保存。如果别人修改了偏移（例如 HiveOS 重新应用超频），
   auto-uv 会在该显卡上退让，不会与之对抗。
6. 退出时（Ctrl+C、SIGTERM、HiveOS `miner stop`、关闭 Windows 控制台窗口、正常结束）在所有 GPU 上恢复你的偏移
   （1.6.1 之前 HiveOS 的 `miner stop` 可能只恢复 GPU0）。如果矿工被强制终止（`kill -9`、崩溃），下次启动会先根据恢复文件
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
     `set "UV_MAX="`（留空 = 按显卡：+495，RTX 20xx/GTX 16xx/CMP 50HX +225，CMP 30HX/40HX +150；RTX 40/50 常能稳定在 +450..480，RTX 30 通常
     +200..360）。逗号分隔的列表按 `DEVICES` 顺序为每卡设置一个值，例如 `set "UV_MAX=225,495,150"`。
   * `start-nvidia-smi.bat`：`AUTO_UV=1`，范围在 `UV_MIN` / `UV_MAX`，并通过 `nvidia-smi` 设置功耗限制和锁频。
   * 自己的命令行：`freeforgem.exe --pool ... --wallet ... --auto-uv --uv-max 480`。
4. **起点。** 调校从显卡当前的偏移开始，且不会低于它。MSI Afterburner（或类似工具）中的 *Core Clock* 偏移就是同一个偏移：
   你在那里设置的值就是起点，退出时会恢复。挖矿期间不要让 Afterburner 重新应用配置文件：auto-uv 发现偏移被别人修改后会在
   该卡上退让。未设置偏移时，调校从 +120 开始，RTX 40 从 +200、RTX 50 从 +300 开始（或 `--uv-min` / `UV_MIN`）。
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

**GPU 频率控制（可选，基于 NVML；需要 root / 管理员权限，Windows 上可用 — 1.4.9 新增）。** `--lock-cclock N` 将核心频率锁定为 N MHz（类似 `nvidia-smi -lgc N,N`），`--lock-mclock N` 将显存频率锁定为 N MHz（类似 `-lmc`），`--moffset N` 设置显存频率偏移（MHz），可以为负数（需要驱动 555+）。每个选项可对所有 GPU 使用同一个值，也可按 `--devices` 顺序给出逗号分隔的列表（每个设备一个值）。设置在启动时应用，退出时（Ctrl-C / SIGTERM / 崩溃）自动撤销。Windows 上请在 `.bat` 文件中设置 `LOCK_CCLOCK`、`LOCK_MCLOCK`、`MOFFSET`。示例：`--lock-cclock 1500 --lock-mclock 810 --moffset -200`。

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
| Kavatar（kavatar.kasplay.online，1 %） | `cmfd+tls://188.35.20.44:29445?pin=71ab1d2e36cd20226ac16184b91dda5527126fa8b29ee1de2b6f5cd8b7028041` |
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
`--uv-offset N` 则设置为 N —— **风险自负**。`--auto-uv` 仅用于 Pearl。安全的做法：默认频率、功耗限制或锁定核心频率（`--lock-cclock`）。

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

**算力**（Aria 与 Kavatar 矿池，5–30 分钟运行，偏移 0，自动批量）：RTX 4070 Ti（功耗限制 190 W）36.8 FW/s @ 187 W；
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

RTX 3070，显存 810 MHz，功耗上限 135 W，每个点 60 秒（`eff` = 每个 SM 512 线程，`speed` = 更多 S-box 运算放在 CLMAD 上）：

| 核心频率 / 偏移 | eff：MH/s @ W（MH/W） | speed：MH/s @ W（MH/W） |
|---|---|---|
| 1400 MHz / +220 | **35.69 @ 76.4（0.467）** | 38.68 @ 85.3（0.454） |
| 1560 MHz / +205 | 39.30 @ 94.6（0.415）（n2gc） | 42.79 @ 105.1（0.407） |
| 1650 MHz / +220 | — | **45.25 @ 121.0（0.374）** |

* **eff**（默认，每瓦 MH 最高）：核心锁定 **1400 MHz** 并使用 `--auto-uv`。
* **speed**（MH/s 最高）：核心 **1560–1650 MHz**（RTX 30）；在 1650 MHz 时比我们在同一张卡上测过的最快闭源矿工
  （1560 MHz 下 42.55 MH/s）更快。
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
* RTX 3070 实测边缘（压力检查，1400–1650 MHz）：最后成功 +220…+235，首次失败 +235…+250 → 结果 +205…+220。低于约 1400 MHz
  时显卡已处于最低电压，偏移不再省电。

**日志（NOID）**

```
FreeForGem 1.7.0-beta | NOID (parano1d) | pool stratum+ssl://eu2.innovlab.cc:19601 | worker rig1 | 1 GPU(s) [0] | mode eff
[noid-pool:user] subscribed: parano1d-stratum-v1, nonce_bits 64, session namespace 24e67d0000000200
new job 46f6...0000 [user] height 239348 (share target 0000003fffffffff.., expires in 262s)
[  60s] GPU0 35.690 (76 W, 0.467 MH/W) | total 35.690 MH/s, 76 W, 0.467 MH/W | hits 412 (bad 0, stale 0) | user sub 3 acc 3 rej 0 | fee sub 0 acc 0 rej 0
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
   参考值比对（1.7.0-beta 中 CMFD 以偏移 0 运行；CMFD 调整器单独发布）。
2. **第一个失败的步骤标记边缘。** 结果取**再低一步**（15 MHz 余量；`--uv-keep-last` 保留最后一个成功步骤），并用 2 分钟运行
   **确认**；确认失败则再降一步。
3. **挖矿期间持续检查结果：** 每个 Pearl 命中和每个 NOID 候选在提交前都在 CPU 上重新计算；NOID 还每 120 秒进行一次看门狗
   检查；Pearl 每 6 小时重新验证。检查失败时**自动将偏移回退一步**（并保存），挖矿继续。
4. 机主的偏移在第一次修改前写入恢复文件，并在**退出时恢复**（崩溃后则在下次启动时恢复）。

日志中的样子（NOID，RTX 3070，`--lock-cclock 1560`，示例）：

```
auto-uv (noid): GPU0 (bus 0d, RTX 3070) base 0: ok (3020 launches checked, 37.10 MH/s, 112.4 W, 0.330 MH/W, 1560 MHz, 61 C)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) +205: ok (4012 launches checked, 37.57 MH/s, 109.4 W, 0.343 MH/W, 1560 MHz, 63 C)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) +220: ok (4010 launches checked, ...)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) +235: FAILED (WRONG RESULTS: 3 of 1046 launches gave a wrong candidate set, first after 9.8s (no Xid, no crash: the silent edge)): edge found
auto-uv (noid): GPU0 (bus 0d, RTX 3070) confirmation +205: ok (...)
auto-uv (noid): GPU0 (bus 0d, RTX 3070) RESULT eff: offset +205 (edge +235, one step below the last good one); MH/W 0.330 -> 0.343 (+3.9%)
...
GPU0: WATCHDOG: 1 of 6 check launches gave WRONG results (silent undervolt edge?)
auto-uv (noid): GPU0: wrong GPU results at offset +205 (CPU check of a hit / watchdog): stepped back to +190 and mining on
```
