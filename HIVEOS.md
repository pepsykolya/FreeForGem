# FreeForGem on HiveOS

[English](#english) · [Русский](#русский) · [中文](#中文)

## English

FreeForGem runs on HiveOS as a **Custom miner**: no SSH and no manual installation.

**Moving from FreeForGemPear:** since 1.7.0 the multi-coin miner is **FreeForGem** (custom miner `freeforgem`). HiveOS treats it
as a different miner: **create a new flight sheet** with Miner name `freeforgem` and the URL below (the old `freeforgempear`
flight sheet keeps running the old miner). Saved `--auto-uv` results are taken over automatically on the first start (from
`/hive-config/freeforgempear/uv.json`). FreeForGemPear itself continues as the Pearl miner.

**Installation URL (1.8.1):**

```
https://github.com/pepsykolya/FreeForGem/releases/download/v1.8.3/freeforgem-1.8.3_hiveos.tar.gz
```

HiveOS caches the downloaded archive by its file name: every version has a new file name, so always paste the link of the new release.
A package you build or rename yourself needs **exactly one dash** in its file name (`freeforgem-1.8.3_hiveos.tar.gz`, name-version_hiveos):
with a second dash HiveOS takes a wrong miner name and the miner silently does not start. A new build under the same file name
is not downloaded again: put it in a new folder of the URL, or run `/hive/miners/custom/custom-get <URL> -f` on the rig.

### Flight sheet

1. **Wallet.** In *Wallets*, add your PRL wallet: coin `PRL`, your `prl1...` address. On LuckyPool you can merge-mine NOCK as well:
   use `prl1...+YOUR_NOCK_ADDRESS` as the address (LuckyPool only; see MANUAL.md, section 6).
2. **New flight sheet.**
   - **Coin:** `PRL`.
   - **Wallet:** the wallet from step 1.
   - **Pool:** *Configure in miner*.
   - **Miner:** *Custom*, then press *Setup Miner Config*.
3. **Miner config:**

   | Field | Value |
   |---|---|
   | Miner name | `freeforgem` (filled in automatically after you paste the URL) |
   | Installation URL | the link above |
   | Hash algorithm | `pearlhash` (or leave `----`) |
   | Wallet and worker template | `%WAL%.%WORKER_NAME%` |
   | Pool URL | `pearl-eu2.luckypool.io:3360`, or Kryptex `prl-eu.kryptex.network:7048` (or any other Pearl stratum pool) |
   | | Kryptex: the wallet may also be your Kryptex username (payouts in BTC/USDT with auto-conversion, since 1.7.4) |
   | Pass | leave empty |
   | Extra config arguments | optional, for example `--devices 0,1`; `--auto-uv` enables the undervolt tuner (see below) |

4. **Apply.** *Apply changes*, *Create flight sheet*, then apply it to the rig.

The miner log is at `/var/log/miner/freeforgem/freeforgem.log` and in the HiveOS miner console.
The dashboard shows the hashrate per GPU and in total, temperatures, fans, and accepted and rejected shares; since 1.7.2 the rig
efficiency (TH/W for Pearl, MH/W for NOID) next to the miner version; since 1.7.5 a newer release too (`1.7.8 0.652 TH/W,
need update!!`; `--no-update-check` turns the check off).

### Overclocking (RTX 3070)

Pearl does not need a high memory clock:

| Setting | Value |
|---|---|
| Core clock | absolute, `1560` MHz |
| Memory clock | absolute, `810` MHz (in HiveOS: Memory clock `810`, or the lowest your card accepts) |
| Power limit | `120` W |

- **1560 MHz:** ~70 TH/s at ~107 W.

### Undervolt (optional, `--auto-uv`, new in 1.4.0)

Off by default. Add `--auto-uv` to *Extra config arguments* (for example `--devices 0,1 --auto-uv`) and the miner finds, once per
GPU, the lowest core voltage (V/F offset) at which the card still computes correctly, applies it while mining and puts your
own settings back when it stops. Same hashrate, fewer watts (RTX 3070 at 1560 MHz: roughly 5-10 W less per card).

- It needs root, which HiveOS miners have. Without NVML or permission it logs the reason and mines normally.
- **The first tuning takes a few minutes** (since 1.8.1, all GPUs at once; a 4x RTX 3070 rig: NEB 1.5-2 min, PROX about 5, Ting 6-8.5,
  NOID about 9, CMFD 10-18). The rig mines meanwhile, on every coin, also the GPU being tuned: its hashrate is somewhat lower only
  while a step is checked, and its shares go to the pool. The result is kept in `/hive-config/freeforgem/uv.json` (survives miner updates and reboots), **one
  profile per core clock lock** (since 1.7.2): switching the lock in the OC profile reuses that lock's verified result, a new lock is
  tuned once; it is tuned again when the driver or the kernel build changes. A GPU that crashes while mining is named in the log and
  stepped back by the miner itself (`gpu-faults.json`; Linux / HiveOS: it restarts itself in the same process, at most 12 times an
  hour), see [MANUAL.md](MANUAL.md#17-profiles-per-clock-lock-mining-during-tuning-crashes-while-mining-new-in-172).
- Set your Overclock tab (core lock `1560`, memory, power limit) first. The tuner starts from the offset of the OC profile
  (RTX 3070: from a default for the core lock); an offset another miner left on
  the card is replaced by the OC profile value at start; it never touches locked clocks, power limit or memory. When HiveOS re-applies its OC while mining (at miner
  start or after an OC change it writes the *Core Clock* value, **0 when the field is empty**), auto-uv puts the verified offset
  back after a quick check (since 1.7.1; before, the undervolt was off until the next restart). A different offset set by hand
  makes it stand down on that GPU.
- `--lock-cclock` / `--lock-mclock` in *Extra config*: on exit the miner puts back the lock of your HiveOS OC profile (since
  1.7.1; before, it removed the lock and the rig ran unlocked).
- It touches only the GPUs the miner mines on (matched by PCI bus id, shown in the log). Every change is logged as
  `auto-uv: GPU0 (bus 0d, RTX 3070) offset +150 -> +195, 116.6 W -> 106.6 W, saved`; `grep auto-uv` in the miner log shows all of it.
- Stopping the miner restores your offset. If the miner was killed, the next start restores it first.
- The tuning starts close to the expected result, so it is short;
  `--uv-min N` sets the starting offset. The miner screen shows the progress per GPU and colours (`--no-color` to turn off).
- **Reset:** add `--uv-reset` for one start: it forgets the profile of the current lock only (`--uv-reset-all`: every profile;
  or delete `/hive-config/freeforgem/uv.json`). On exit the offset of the OC profile (*Core Clock*) is put back.
  Since 1.8.1 `--uv-reset`, `--uv-reset-all`, `--uv-unpause` / `--uv-parallel-reset` and `--auto-lock-reset` act **once, at a start
  you make** (a new flight sheet, `miner restart`, a reboot after a clean stop); a restart after a fault skips them. Take them out
  of the flight sheet after that start, and **never keep `--uv-unpause` in the flight sheet**: it would lift the restart guard at
  every start.
- **Mining during the tuning and guards, since 1.8.1 (every coin).** Before 1.8.1 the card stopped mining at every tuning step (Pearl: up to
  about 14 minutes without hashrate, the pool did not see the worker); the Pearl warm-up mines too now. All GPUs are tuned at once, as many as the
  rig's free RAM allows; after trouble of the whole rig (a GPU lost from the bus, a hanging check) 1 at a time, remembered for the rig
  (`--uv-parallel N` sets it by hand, `--uv-parallel-reset` forgets the caution). A failed step goes on 2 steps below the failed offset, and a
  crash while mining restarts the card at once 2 steps below the crashed offset, without a tuning from scratch. Faults are counted per coin: a card
  that keeps crashing is left out for a while (`OUT 14m` in the miner screen, it comes back by itself); a start you make (new flight sheet,
  `miner restart`, a reboot) gives it a new chance. Guards: repeated failures on a card switch its auto-uv off for 24 hours; repeated restarts of the rig
  during a tuning pause it until `--uv-unpause` / `--uv-reset-all` or a new version. The states `[AUTO-UV GPU3 OFF 24h]`,
  `[AUTO-UV PAUSED]`, `[AUTO-UV NOT CALM]` show in the miner screen, the API and HiveOS. MANUAL.md, section 30.
  Other options (`--uv-offset N`, `--uv-max N`, `--uv-reval-hours H`, `--auto-uv-dry-run`) and a manual-tuning
  procedure are in [MANUAL.md](MANUAL.md#12-undervolt----auto-uv-optional-new-in-140). Undervolting is at your own risk, like any overclocking.
- **No offset ceiling of the miner's own (since 1.8.1):** every card climbs as long as each step lowers the power, at most to the range of its
  driver; the old per-card limits are gone. `grep range` in the miner log shows each GPU's range. To set your own limit give one value per GPU
  in nvidia-smi order (GPU0, GPU1, ... as in the miner log; or in `--devices` order), no spaces: `--auto-uv --uv-max 225,225,-,-,-,150`
  (`-` = no limit of yours). A list may be shorter than the number of GPUs: the last value goes to the remaining ones (see *Per-GPU lists* below).
- **Turing (RTX 20xx, GTX 16xx, CMP HX), since 1.8.1:** a result is confirmed longer on its mining load; a card that crashes again soon after goes
  at once to its stable offset; a Turing profile tuned by an older version gets this longer check once.
  MANUAL.md, section 30.
- **Results into the OC profile:** `--hive-oc-sync` (see below) reports the tuned offsets to the HiveOS panel and, with a token, writes them into
  the OC profile.

### Tuned offsets into the OC profile (`--hive-oc-sync`), since 1.8.1

With `--auto-uv`, once the cards are tuned:

- `--hive-oc-sync` sends **one message to the HiveOS panel** (the worker's messages): `FreeForGem auto-uv (Ting): 4 GPUs tuned at 1800 MHz:
  -44 W (-11.8 %)`, with every card's offset, lock and watts before / after, and the lines `Core Clock Offset` / `Lock Core Clock` (with
  `--lock-mclock` also `Lock Mem Clock`) ready to paste into the OC form. Sent again only when the results change. Nothing is written.
- With a **HiveOS API token** the miner also writes the confirmed offsets and the core / memory clock locks it sets (`--lock-cclock` /
  `--auto-lock`, `--lock-mclock`) into this worker's OC profile (the flight sheet's algo, else *Default Config*); HiveOS applies it on the rig.
  The rest of the profile (power limit, fans, other algos) is not touched; a copy is kept before every write. With `--split` every group writes only its own cards and sends its own message.
- **The token:** HiveOS web → account settings → *Sessions* / API → a new token **just for this**. Put it into *Extra config arguments*:
  `--auto-uv --hive-oc-sync --hive-oc-sync-token YOUR_TOKEN`. The miner masks the token on its screen and in its log.
- **Or keep it out of the flight sheet:** a token in the flight sheet is seen by everyone with access to the farm, and on the rig in `ps` and
  in the rig's config. If you prefer not to have it there: on the rig `nano /root/ffg-hive-token` (paste, save),
  `chmod 600 /root/ffg-hive-token`, and in *Extra config arguments* `--auto-uv --hive-oc-sync --hive-oc-sync-token-file /root/ffg-hive-token`
  (not in `/hive-config`: files there are readable by every user).
- `--hive-oc-sync-test` sends one test message and exits. A rejected token (401/403): an orange line, no more writes until the next
  restart, the messages go on. MANUAL.md, section 32.

### Automatic core lock (optional, `--auto-lock`, new in 1.7.8)

Add `--auto-lock` to *Extra config arguments* (Pearl, NOID, PROX; not CMFD). The miner then sweeps the core clock locks of every GPU
(default 1800 ... 1200 MHz, each with its own `--auto-uv` profile, measured while really mining: the shares go to the pool),
keeps the best TH/W within 10 % of the top hash rate and saves it in `/hive-config/freeforgem/autolock.json`: later starts apply it at once.
The first sweep takes 30-90 min (all GPUs at once); the hash rate shows on the dashboard all the time, with short gaps between rounds.
Options: `--auto-lock-range 1300-1600:100`, `--auto-lock-max-loss 10`, `--auto-lock-goal eff|speed`, `--auto-lock-reset` (sweep again).
A GPU with its own `--lock-cclock` value is not swept. Details: MANUAL.md, section 23.

**By money:** `--auto-lock --auto-lock-goal profit --power-price 0.12,0.053@23-07` (electricity $/kWh, day and night 23-07 h) keeps the
lock with the most US$ per day (income from the network data and the coin price minus electricity at the measured watts); every lock
2000 ... 1200 is measured once. The price comes from public tickers (GET only, nothing of yours is sent) or `--coin-price X` (no exchange
is asked); every `--auto-lock-reprice-hours` (6) the choice is made again from the measured points (switch only for > 3 % more,
`--auto-lock-hysteresis`). `--pool-fee PCT` for a pool the miner does not know. MANUAL.md, section 23.1.

### Requirements

- HiveOS with NVIDIA driver 525 or newer.
- Tuned for RTX 30xx and RTX 40xx (own Ada kernel since 1.5.4); RTX 20xx/GTX 16xx/CMP and RTX 50xx are supported too (CMP 50HX: ~67 TH/s per card at lock 1300 and ~77 TH/s at 1500 on driver 610.43.03, 166-176 W; only ~2.6 TH/s on the older drivers 550.78 / 595.91.07).
- Older GPUs are not supported. AMD Radeon cards are supported on Windows only (since 1.6.0); the HiveOS package is NVIDIA-only.
- **RAM:** mining needs about 0.2 GB plus 0.1-0.2 GB per GPU, and up to ~1.5 GB more for a moment while it proves a Pearl share
  (since 1.7.3). `--auto-uv` checks need ~0.25 GB each plus ~1.6 GB shared by all of them (RTX 3070 / CMP 50HX). With little free
  RAM the miner checks fewer GPUs at a time (log: "checking N of M GPU(s) at a time"), and when even one does not fit (a 4 GB rig) it runs no checks or tunings: a GPU with a verified profile of
  its clocks uses it, the others mine at your offset. A check process is the first one the Linux OOM killer takes; a check killed
  for lack of RAM is not counted as a failed offset. Without `--lock-cclock` / `--lock-mclock` the miner sets the OC profile's
  locks before mining (another program may have left the clocks unlocked), and the GPUs start one by one, 1.5 s apart.
- **HiveOS reboots the rig at a high load average, even with its watchdog off** (`/hive/bin/wd`, check_la: `sreboot fast` when the
  5-min load average reaches the rig's max_la, 28 on a 4C/8T Xeon rig). Weak CPUs (4 cores) with many GPUs are at risk while
  several GPUs tune at once. Since 1.7.2 the miner keeps its CPU load low and the tuning summary shows the load average, with a WARNING above 2 x the CPU threads.

### CMP cards: unlocked driver (optional, off by default, 1.7.7)

Under the stock NVIDIA driver a CMP card (CMP 50HX and the family) makes about a tenth of its speed. To let the miner install an
unlocked driver, add the word `cmp-unlock-driver` to *Extra config arguments* (the miner does not get it). Before mining it checks, installs the driver
only if the unlocked build is not in place yet, and **reboots the rig**; a rig that already has it, has no CMP card or runs without root is left
alone. **This replaces the NVIDIA kernel driver of the whole rig:** read MANUAL.md section 20 first. By hand: `sudo ./freeforgem --cmp-driver`
(`FFG_CMP_DRIVER_DRY_RUN=1` or `--cmp-driver --dry-run` only reports what it would do; a rig unlocked by rgminer is left alone).

### Backup pools (failover and failback), since 1.7.8

The flight sheet's **Pool URL** field takes several URLs of one coin, separated by a space or a comma; the first one is the primary:

    pearl-eu2.luckypool.io:3360 prl-eu.kryptex.network:7048

If a pool cannot be reached, refuses the login or sends no job, the miner moves to the next URL; while it is on a backup it checks the
primary every 10 minutes on a separate connection (login + a job; `--failback-minutes N` in *Extra config arguments*, `0` = off) and
goes back after 2 good checks in a row, without losing the shares in flight. The dev fee follows you to the pool you are on. CMFD: the
engines are restarted on the next pool (about a minute without mining per switch). `--pool-check` in a shell (`./freeforgem --pool "A,B"
--wallet W --pool-check`) tests every entry without a GPU. Details: MANUAL.md, section 22. A `http://` solo URL is one node: no list.
`--noshare-minutes M` (15) and `--reject-minutes K` (5) in *Extra config arguments* (0 = off): a pool that takes the connection but credits no
share of yours (while the GPUs hash and shares were expected) is reconnected, then the next pool of the list is used.
Since 1.8.1 a pool session that went bad is replaced by a new one (NEB, NOID, PROX, Ting, Pearl stratum): when the pool drops the share
difficulty or answers late with many stale shares. The log and the miner screen say `pool: reconnecting: <why>` (yellow).

### Developer fee

1%: 60 s in every 100 minutes, on the same pool you set (unchanged in 1.8.1). See [LICENSE](LICENSE).
Since 1.8.0 every coin's fee runs on a connection of its own: NEB on your pool list (solo / gate: the public NEB pools), Ting on
crypto-connection.net, PROX a second login on your pool (proximamine only as the fallback), CMFD inside the engine (36 s per hour per GPU). With `--split` every group pays 1 % of
its own cards' time. A slot whose connection was not ready is tried again up to 3 times.

### NOID (Parano1d), since 1.7.0

Needs NVIDIA RTX 30/40/50 and **driver 580+** (`nvidia-driver-update 580`). Flight sheet: coin `NOID`, wallet `o1...`, Miner
*Custom* `freeforgem` (same Installation URL), Hash algorithm `parano1d`, template `%WAL%.%WORKER_NAME%`, Pool URL
`stratum+ssl://eu2.innovlab.cc:19601`, Extra config `--algo noid --lock-cclock 1300 --auto-uv` (eff mode, best MH/W on RTX 30;
`--noid-mode speed --lock-cclock 1500 --auto-uv` for ~31 % more MH/s at ~4 % lower MH/W; 8x RTX 3070 table: MANUAL.md, section 15). Without `--auto-uv` a positive HiveOS core offset is set to 0 for NOID
(a Pearl undervolt gives silently wrong NOID results) and put back when the miner stops. Results: MANUAL.md, sections 15 and 16.
At the start the innovlab vardiff begins with an easy share target and the pool's replies can lag several seconds; since
1.7.2 the miner limits the shares waiting for a reply and skips surplus easy ones (stats line: `rej N (stale N, skipped N)`);
a block share is always sent.

### Common Foundry (CMFD), since 1.6.0

The same package mines CMFD (engine: FreeForgeMiner, MIT, in `cmfd/`). Flight sheet: *Hash algorithm* `forgematrix_v4`,
*Wallet and worker template* `%WAL%.%WORKER_NAME%` with your CMFD address (`cmfd1...` or 64 hex) as the wallet, *Pool URL*
Aria `cmfd+tls://159.69.194.46:29445?pin=9dfb51083f287726117f689f87bc7a878792efcca58ca8b6f6e7c05ac5d152e9` or
Midas Mining `cmfd+tls://24.107.152.151:29745?pin=2ee4e6a028b07b300753dff3c260b79763c5e748d4e78b86cf3855e97eb2e80e`
(other pools: MANUAL.md section 13). Optional *Extra config*: `--cmfd-mode eco`, `--devices 0,1`, `--per-gpu-workers`.
The 6.4 GB model goes to `/hive/miners/custom/cmfd-model` (shared with FreeForgeMiner; downloaded once).
Stock clocks or a power limit: a positive core offset of the OC profile is set to 0 by the miner for the run (and put back);
`--uv-offset N` forces a value at your own risk. Dev fee 1% inside the engine (36 s per hour per GPU, worker `ffm-fee`).
**Use a separate OC profile for CMFD without the memory lock.** The Pearl OC (memory locked at 810) cuts CMFD about 6×:
an RTX 3070 gave 3.7 FW/s with *Mem lock* 810 and 21.8 FW/s (~237 W, power limit 240 W) with the memory free. The miner
warns about a low memory clock in the log (once per GPU, since 1.6.1) but does not change your OC.
CMFD runs on HiveOS images based on Ubuntu 20.04 and 22.04 alike (the engine in the package needs glibc 2.30+).
`--auto-uv` in *Extra config* tunes a separate CMFD core offset per card on the CMFD load itself (every step checked
bit-for-bit; first start ~15–20 min, the card mines meanwhile (since 1.8.1), then applied at once; owner's offset back on `miner stop`). Tune with the
OC profile you mine with: a changed power limit re-tunes; a changed core clock lock picks (or tunes once) its own CMFD profile (since 1.7.2). MANUAL.md, section 13.

### Proxima (PROX), since 1.7.8

Needs an NVIDIA card (GTX 10 / RTX 20/30/40/50). Flight sheet: coin `PROX` (add it as a custom coin if it is not in the list), wallet =
your PROX address (InnovLab: your holder id, 64 hex), Pool *Configure in miner*, Miner *Custom* `freeforgem` (same Installation URL).
**Hash algorithm: leave `----`**: HiveOS has no `proxima` in its list. Template `%WAL%.%WORKER_NAME%` (solo: `%WORKER_NAME%`).

| | proximamine | InnovLab | solo |
|---|---|---|---|
| Pool URL | `stratum+ssl://pool.proximamine.com:3334` | `innov+tls://eu3.innovlab.cc:19701` | `http://NODE_IP:PORT` |
| Pass | your proximamine secret | `x` | the node's `mine.seeker.token` |

Screenshots of every field: `docs/img/hiveos-prox-*.png` (also in MANUAL.md, section 21).
Extra config: `--auto-uv --lock-cclock 1500` (RTX 3070, RTX 3070 Laptop, RTX 3080 Ti; one value for every card or a comma list per GPU).
The core lock sets the speed and the auto-uv offset the watts; recommended locks (offset found or fixed): RTX 3070 1500 (+120,
2.66 GH/s, 87 W), RTX 3070 Laptop 1500 (+270, 2.23 GH/s, 53 W), RTX 3080 Ti 1500 (+210, 4.40 GH/s, 147 W; for speed 1800 +270, 5.28 GH/s,
201 W), RTX 4070 Ti **2450** (+270, 5.47 GH/s, 142 W). CMP 50HX is not measured (no memory lock on CMP). The memory is locked at its lowest
clock by default (`--no-mem-lock` leaves it). The HiveOS OC of the web UI is applied by `nvidia-oc` at every miner start and OC change and
resets the lock and the offset: since 1.7.8 the miner sets them again (log: `... was reset by an OC apply ... set again`, at most 4 times per
GPU per hour). The dashboard shows GH/s and GH/W. Pools, options and the dev fee (1%): MANUAL.md, section 21.

### NEB (Neural Engine Byte), since 1.8.0

Flight sheet: coin `NEB` (add it as a custom coin if it is not in the list), wallet `neb1...`, Pool *Configure in miner*, Miner
*Custom* `freeforgem` (the same Installation URL):

| | Suprnova | your gate (several rigs on one NEB node) |
|---|---|---|
| Hash algorithm | `nebhash` | `nebhash` |
| Wallet and worker template | `%WAL%.%WORKER_NAME%` | `%WAL%.%WORKER_NAME%` (the blocks pay the node's `--miner-lock`) |
| Pool URL | `stratum+tcp://neb.suprnova.cc:7804` (backup: `stratum+tcp://neb.suprnova.cc:7804,stratum+tcp://neb.suprnova.cc:7805`) | `neb://GATE_IP:3442` |
| Pass | `x` | the gate token |
| Extra config arguments | e.g. `--auto-uv --lock-cclock 1800 --table-mclock 810` | same |

- The URL selects NEB; `--algo neb` in Extra config forces it for another NEB pool.
- The OC profile may keep Pearl's memory lock (810 MHz): while NEB mines the memory runs at its maximum and the profile's lock is
  put back when the miner stops. Do not lower the memory for NEB. `--table-mclock 810` lowers it only where NEB does not need
  the fast memory (RTX 3070: -10 W, +8-10 % MA/W); `--tbig auto` (default) uses more GPU memory where it pays back.
  A model that needs the full memory clock keeps it, and that is right (at 810 MHz it would lose most of its hashrate). Since 1.8.1
  such a model raises no `NO TABLE` alarm; the miner screen shows `/810` in cyan while the lock is in use.
- Engine nebgpu 0.8.4 (since 1.8.1; faster on Turing), one engine process per GPU: a CUDA fault on one card restarts only that card (HiveOS shows one more
  `freeforgem` process per GPU).
- `--power-limit N` (`--pl N`) is optional and never on by default: the miner limits the power with the core lock and the offset.
  `--reboot-on-lost-gpu 10` (opt-in) reboots the rig when a GPU has been lost (fallen off the bus) for 10 minutes.
- HiveOS shows khs (MA/s x 1000), the 15-minute rate, MA/W next to the version, `LOCK NOT HELD`, `NO TABLE`, `GPU LOST`,
  `REBOOT NEEDED` and NEB memory warnings.
- A rig that mined NEB with `ffgn-hive` keeps its tuned offsets (`/hive-config/ffgn/uv.json` is imported on the first start).
- The whole rig on NEB = this flight sheet; NEB on some cards and another coin on the others: *Different coins on different
  cards* below. Details: MANUAL.md, section 24.

### Ting / Daler (DLR), since 1.8.0

The Ting miner FFGT is part of FreeForGem since 1.8.0 (same kernels, same numbers). NVIDIA GTX 10 and newer (CUDA), AMD (OpenCL).
Flight sheet: coin `DLR` (add it as a custom coin), wallet = your Ting account (64 hex digits, **without** `&net=...`; Ting ID app
or `ting wallet account`), Pool *Configure in miner*, Miner *Custom* `freeforgem` (same Installation URL).
**Hash algorithm: leave `----`** (no `ting-sha256` in the HiveOS list). Template `%WAL%.%WORKER_NAME%`.

| | crypto-connection.net | your ffgt-gate | solo on a node of this rig |
|---|---|---|---|
| Pool URL | `crypto-connection.net:4073` | `ting://GATE_IP:PORT` | `node://127.0.0.1:18440` |
| Pass | `x` | the rig's gate password | - (Extra config `--node-dir DIR`) |

Extra config: `--lock-cclock 1800 --auto-uv` (RTX 3070: 1500 = best GH/W, 1800 = more GH/s). The memory is locked at its lowest clock by default (SHA-256 does not use it: about -10 % power; `--no-mem-lock` leaves it; a
memory lock of the HiveOS OC profile is kept; CMP 50HX: the driver has no memory lock). Grouped search (default) gives about +40 % solo and
+25 % on the pool on RTX 30 (`--no-group` turns it off). The dashboard shows GH/s and
GH/W, algo `ting`. Each GPU mines in an engine process of its own: a CUDA fault on one card restarts only that card in a few
seconds (HiveOS shows one more `freeforgem` process per GPU; `--single-process` = the old mode). Dev fee 1 % (60 s per 100 min) on
crypto-connection.net. Options: `freeforgem --algo ting --help`. Details: MANUAL.md, section 26.

### Different coins on different cards (`--split`), since 1.8.0

One flight sheet for the whole rig: the **main coin** is set in the flight sheet as usual (its Pool URL, Wallet and Pass), the
**other coin** goes into *Extra config arguments* with values tagged by its name (`--pool pearl=...`, `--wallet pearl=...`). A
tagged value replaces the flight sheet's untagged value for that coin's group, so the flight sheet's pool and wallet stay with
the main coin.

**Example: GPU0 and GPU1 mine NEB on Suprnova, GPU2 and GPU3 Pearl on Kryptex** (replace the placeholder wallets with yours):

1. **Wallets.** A NEB wallet (`neb1...`). The PRL address goes into Extra config, so it needs no wallet entry in HiveOS.
2. **Flight sheet:** coin `NEB`, the NEB wallet, Pool *Configure in miner*, Miner *Custom* → *Setup Miner Config*:

   | Field | Value |
   |---|---|
   | Miner name | `freeforgem` |
   | Installation URL | the link above (1.8.1) |
   | Hash algorithm | `nebhash` |
   | Wallet and worker template | `%WAL%.%WORKER_NAME%` |
   | Pool URL | `stratum+tcp://neb.suprnova.cc:7804` |
   | Pass | `x` |
   | Extra config arguments | `--split neb:0,1 pearl:2,3 --pool pearl=prl-eu.kryptex.network:7048 --wallet pearl=prl1YOUR_PRL_ADDRESS --lock-cclock neb=1800 --lock-cclock pearl=1410 --table-mclock neb=810 --auto-uv` |

3. **Overclocking:** the OC profile may lock the memory of all cards at 810: the NEB cards run at the full memory clock while they
   mine (and get 810 back on exit), the Pearl cards keep it. The core locks come from `--lock-cclock neb=...` / `pearl=...`.
4. **Apply** the flight sheet to the rig.

Rules for Extra config:

- **The groups:** `COIN:GPUS` with the GPU numbers of the dashboard (`neb:0,1 pearl:2,3`, ranges `neb:0-3`, one group may take
  `rest`: `neb:0,1 pearl:rest`). Spaces, `,` and `;` are equal separators, no quotes needed. One card mines one coin.
- **Untagged options go to every group** (`--auto-uv`, the worker of the template); **tagged** ones only to their coin's group
  (`--lock-cclock pearl=1410`). Options that only some coins have (NEB `--table-mclock`, `--pl`, `--tbig`; Ting `--no-group`; ...)
  go to those coins' groups by themselves; the miner log has one `split:` line per group with what it left out.
- **A per-GPU list without a tag is for the whole rig** and is sliced per group: `--lock-cclock 1800,1800,1410,1410` gives NEB
  1800,1800 and Pearl 1410,1410.
- **HiveOS splits Extra config at spaces**, so `--split-args` takes one word: write `--split-args neb=--auto-uv` once for every
  flag of one group, and options with a value as tags (`--table-mclock neb=810`, not `--split-args neb="--table-mclock 810"`).
- The flight sheet's Pool URL, Wallet and Pass (and a solo node) are tagged with the flight sheet's coin by the miner's HiveOS
  scripts, so they reach only that coin's group: a gate password never goes to the other coin's pool.
- Tag a value (`--table-mclock neb=810`) to be explicit, or to give two groups different values.

What the dashboard shows:

- the **algo and the total** of the group with the most cards (of groups with as many cards, the first one in `--split`);
- **every card** with its own rate; when the coins have different units, every card is given as its plain rate per second (NEB
  attempts/s, Pearl hashes/s), so do not compare those numbers across coins;
- **accepted / rejected** shares of all groups together;
- the **version column** lists every group with its efficiency: `1.8.1 split: neb 1.40 MA/W + pearl 0.61 TH/W`, plus tags such as
  `LOCK NOT HELD` or `GPU LOST` of the group they belong to.

Every group runs as its own miner process with its own pool session, dev fee, clocks, memory policy, tuners and restarts. A group
that ends restarts on its own after 10 s (after 3 ends within 10 min: a 10-minute pause) while the other groups keep mining.
`miner stop` stops every group, and each puts its own clocks back. In the miner log (`/var/log/miner/freeforgem/freeforgem.log`)
every line of a group starts with `[neb]` / `[pearl]` (light blue in the miner screen), the supervisor's own lines with `split:`.
Tested on a 4-card HiveOS rig (2 NEB + 2 Pearl). Details: MANUAL.md, section 27.

### Per-GPU lists of any length, since 1.8.0

Every per-GPU option (`--lock-cclock`, `--lock-mclock`, `--moffset`, `--uv-offset`, `--uv-max`, `--uv-min`, `--temp-limit`,
`--temp-stop`, NEB `--table-mclock` and `--power-limit`) takes a list of any length, like the OC lists of HiveOS: a shorter list
gives its last value to the remaining GPUs, values past the last GPU are ignored, `-` / an empty field / `0` where 0 is not
valid = that GPU is left alone. So one flight sheet fits rigs with different numbers of cards: on an 8-card rig
`--table-mclock 810,810,-,-` = 810 on GPU0 and GPU1, the other six untouched (before 1.8.0: "4 values for 8 device(s)" and no
start). MANUAL.md, section 28.

### The miner screen: a table of the GPUs, since 1.8.1

Open Hive Shell (or SSH) and type `miner`: instead of the scrolling log the miner shows a table of the GPUs: hashrate, temperature,
fan in %, watts, power limit, per watt, clocks and locks, the V/F offset and the shares of each GPU, the rig total, and on top the
pool and worker, `net` (block, job, ping, stale), `mode` (the algorithm, auto-uv, locks), your options (`args`, secrets masked) and
the latest important events (`!`, a fixed block of 5 lines) and live alarms (`!!`). The OFFSET cell: orange `► +300 ↑ 3/9` while
auto-uv tunes, orange `✗ -2` / `× roll-back` for a failed step and its roll-back, green `✓ -9.6%` when tuned, yellow `-` when auto-uv
is off; `OUT 14m` = a card left out after repeated faults, it comes back by itself. Orange is the tuning doing its job, red needs
your hands. The meaning of every column, mark and colour: MANUAL.md, section 31.

- `miner log`, the web "Miner log" and h-stats.sh read the log as before: every line is there, unchanged.
- h-run.sh puts the miner window in UTF-8 and turns the screen log of that window off, so `/run/hive/miner.N` gets the log lines, not
  the redrawn screen. Under GNU screen there are 16 colours: orange looks dim yellow (brown-orange in the web Hive Shell), ✗ ✓ are × √.
- Options in *Extra config arguments*: `--ui lines` the old scrolling log; `--ui ascii` if the box looks broken (odd letters instead of
  lines); `--no-color` no colours; `--ui-demo split` in a shell shows the table with a made-up rig.
- BAD counts the wrong results of a GPU **while mining** that the CPU check caught (never sent; not bad shares: those the pool refused
  are REJ). Wrong results at an auto-uv step are the guard's work: the OFFSET cell shows them for the time of that step
  (`+390 ✗ 15 wrong`), BAD does not count them.

**The window stays open (since 1.8.1).** After an exit with an error h-run.sh prints `freeforgem exited (code N),
restarting in 5 s` (screen, log, `miner log`) and starts the miner again in the same window; the one-shot flags (`--uv-reset`,
`--uv-reset-all`, `--uv-unpause`, `--uv-parallel-reset`, `--auto-lock-reset`) are skipped at such a start. `miner stop` /
`miner restart` (also the HiveOS watchdog's) end the loop as before. After frequent exits the window waits 10 minutes before
the next start; `miner restart` starts it at once. The window closes ("[screen is terminating]") only on `miner stop` / `miner restart`.

### Faults and lost GPUs, since 1.8.0

- After a crash auto-uv resumes at the crashed offset **minus 2 steps** (every coin).
- After a GPU fault the card is first put into a **safe state** (among others a safe offset), and only then its engine
  starts again.
- **FAULTY:** 3 faults within 10 minutes at the safe offset (NEB, Ting): that card pauses 10 minutes, then gets a new engine. Not
  lost, no reboot needed: check its clocks, power and riser.
- **DEAD / LOST:** only a card that cannot be opened any more, an Xid 79 (fell off the bus) or a card NVML no longer sees: it
  counts 0 on the dashboard (never its last rate, so the HiveOS watchdog sees the real total) and the version column shows
  `GPU LOST` / `REBOOT NEEDED`. Opt-in `--reboot-on-lost-gpu 10` (NEB) reboots the rig after 10 minutes of it (never in the
  first 10 minutes of the miner, at most once an hour).
- **Stale shares:** a job the pool answers "job not found" for is dropped at once; when the pool's answers come seconds late the
  log says `WARNING: the pool answers late` (lost packets on the rig's network: check its network card, cable and switch).

MANUAL.md, section 29.

## Русский

FreeForGem запускается в HiveOS как **Custom miner**: SSH и ручная установка не нужны.

**Переход с FreeForGemPear:** с 1.7.0 мульти-монетный майнер называется **FreeForGem** (кастомный майнер `freeforgem`). Для
HiveOS это другой майнер: **создайте новый полётный лист** с Miner name `freeforgem` и ссылкой ниже (старый лист
`freeforgempear` продолжит запускать старый майнер). Сохранённые результаты `--auto-uv` подхватываются автоматически при первом
запуске (из `/hive-config/freeforgempear/uv.json`). Сам FreeForGemPear остаётся майнером Pearl.

**Ссылка для установки (1.8.1):**

```
https://github.com/pepsykolya/FreeForGem/releases/download/v1.8.3/freeforgem-1.8.3_hiveos.tar.gz
```

HiveOS кэширует скачанный архив по имени файла: у каждой версии новое имя файла, поэтому всегда вставляйте ссылку нового релиза.
В имени пакета, который вы собрали или переименовали сами, должен быть **ровно один дефис** (`freeforgem-1.8.3_hiveos.tar.gz`,
имя-версия_hiveos): со вторым дефисом HiveOS берёт неверное имя майнера, и майнер молча не стартует. Новая сборка под тем же именем
файла заново не скачивается: положите её в новую папку URL или выполните на риге `/hive/miners/custom/custom-get <URL> -f`.

### Полётный лист

1. **Кошелёк.** В разделе *Wallets* добавьте кошелёк PRL: монета `PRL`, адрес `prl1...`. На LuckyPool можно заодно майнить NOCK:
   укажите адрес в виде `prl1...+ВАШ_АДРЕС_NOCK` (только LuckyPool; см. MANUAL.md, раздел 6).
2. **Новый полётный лист.**
   - **Coin:** `PRL`.
   - **Wallet:** кошелёк из шага 1.
   - **Pool:** *Configure in miner*.
   - **Miner:** *Custom*, затем *Setup Miner Config*.
3. **Настройки майнера:**

   | Поле | Значение |
   |---|---|
   | Miner name | `freeforgem` (подставится сам после вставки ссылки) |
   | Installation URL | ссылка выше |
   | Hash algorithm | `pearlhash` (или оставить `----`) |
   | Wallet and worker template | `%WAL%.%WORKER_NAME%` |
   | Pool URL | `pearl-eu2.luckypool.io:3360` или Kryptex `prl-eu.kryptex.network:7048` (или любой другой stratum-пул Pearl) |
   | | Kryptex: кошельком может быть и имя пользователя Kryptex (выплаты в BTC/USDT с автоконвертацией, с 1.7.4) |
   | Pass | пусто |
   | Extra config arguments | по желанию, например `--devices 0,1`; `--auto-uv` включает подбор андервольта (см. ниже) |

4. **Применить.** *Apply changes*, *Create flight sheet*, затем применить лист к ригу.

Лог майнера лежит в `/var/log/miner/freeforgem/freeforgem.log`, его видно и в консоли майнера в HiveOS.
На панели видны хэшрейт по картам и общий, температура, вентиляторы, принятые и отклонённые шары; с 1.7.2 — эффективность
рига (TH/Вт для Pearl, MH/Вт для NOID) рядом с версией майнера.

### Разгон (RTX 3070)

Высокая частота памяти Pearl не нужна:

| Параметр | Значение |
|---|---|
| Частота ядра | абсолютная, `1560` МГц |
| Частота памяти | абсолютная, `810` МГц (или минимальная, которую принимает карта) |
| Лимит мощности | `120` Вт |

- **1560 МГц:** ~70 TH/s при ~107 Вт.

### Андервольт (необязательно, `--auto-uv`, новое в 1.4.0)

По умолчанию выключено. Добавьте `--auto-uv` в *Extra config arguments* (например `--devices 0,1 --auto-uv`), и майнер один раз
для каждой карты найдёт минимальное напряжение ядра (V/F offset), при котором карта ещё считает правильно, применит его при
майнинге и вернёт ваши настройки при остановке. Тот же хешрейт, меньше ватт (RTX 3070 на 1560 МГц: примерно на 5-10 Вт меньше на карту).

- Нужен root, а у майнеров в HiveOS он есть. Без NVML или прав майнер пишет причину в лог и майнит как обычно.
- **Первый подбор занимает несколько минут** (с 1.8.1 — все карты сразу; риг 4x RTX 3070: NEB 1,5–2 мин, PROX около 5, Ting 6–8,5,
  NOID около 9, CMFD 10–18). Риг при этом майнит, на всех монетах, и подбираемая карта тоже: её хешрейт немного ниже, только пока
  шаг проверяется, а шары идут в пул. Результат хранится в `/hive-config/freeforgem/uv.json` (переживает обновления майнера и перезагрузки),
  **отдельный профиль на каждый лок ядра** (с 1.7.2): смена лока в профиле разгона берёт проверенный результат этого лока, новый лок
  подбирается один раз; при смене драйвера или сборки ядра подбор повторяется. Карту, падающую в майнинге, майнер называет в логе и
  сам понижает (`gpu-faults.json`; в Linux / HiveOS перезапускает себя в том же процессе, не больше 12 раз в час), см.
  [MANUAL.md, раздел 17](MANUAL.md#17-профили-по-локу-частоты-майнинг-во-время-тюнинга-падения-в-майнинге-новое-в-172).
- Сначала задайте вкладку Overclock (фиксация ядра `1560`, память, лимит мощности). Подбор стартует с offset профиля разгона
  (RTX 3070 — со значения по умолчанию для лока ядра); offset, оставленный на
  карте другим майнером, при старте заменяется значением профиля; зафиксированные частоты, лимит мощности и память он не трогает.
  Если HiveOS заново применит разгон во время майнинга (при старте майнера и после смены OC он записывает значение *Core Clock*,
  **0, если поле пустое**), auto-uv после быстрой проверки поставит проверенный offset обратно (с 1.7.1; раньше андервольт
  выключался до перезапуска). Другой offset, выставленный вручную, — и auto-uv отступит на этой карте.
- `--lock-cclock` / `--lock-mclock` в *Extra config*: при выходе майнер возвращает лок из профиля разгона HiveOS (с 1.7.1;
  раньше лок снимался, и риг работал без него).
- Затрагиваются только карты, на которых работает майнер (сопоставление по PCI-адресу, он виден в логе). Каждое изменение
  пишется в лог так: `auto-uv: GPU0 (bus 0d, RTX 3070) offset +150 -> +195, 116.6 W -> 106.6 W, saved`; `grep auto-uv` по логу майнера покажет всё.
- При остановке майнера ваш offset возвращается. Если майнер был убит, при следующем запуске он восстанавливается первым делом.
- Подбор начинается рядом с ожидаемым результатом, поэтому он короткий;
  `--uv-min N` задаёт стартовый offset. Экран майнера показывает прогресс по картам и цвета (`--no-color` — без цвета).
- **Сброс:** добавьте `--uv-reset` на один запуск: он забывает только профиль текущего лока (`--uv-reset-all` — все профили; или
  удалите `/hive-config/freeforgem/uv.json`). При выходе возвращается offset профиля разгона (*Core Clock*).
  С 1.8.1 `--uv-reset`, `--uv-reset-all`, `--uv-unpause` / `--uv-parallel-reset` и `--auto-lock-reset` действуют **один раз, при
  вашем старте** (новый полётный лист, `miner restart`, перезагрузка после чистой остановки); перезапуск после сбоя их пропускает.
  После такого старта уберите их из листа, а **`--uv-unpause` никогда не держите в листе**: он снимал бы защиту от перезапусков на
  каждом старте.
- **Майнинг во время подбора и защита, с 1.8.1 (все монеты).** До 1.8.1 на каждом шаге подбора майнинг карты останавливался (на Pearl до ~14 минут
  без хешрейта, пул не видел воркер); прогрев Pearl теперь тоже майнит. Все карты подбираются сразу, сколько позволяет свободная память рига; после
  неприятностей всего рига (карта отвалилась от шины, зависшая проверка) — по одной, и риг это запоминает (`--uv-parallel N` задаёт вручную,
  `--uv-parallel-reset` забывает осторожность). Шаг, который не прошёл, продолжается на 2 шага ниже сбойного offset, а падение в майнинге сразу
  перезапускает карту на 2 шага ниже упавшего offset, без подбора с нуля. Сбои считаются по монете: карта, которая падает снова и снова, на время
  исключается (`OUT 14m` на экране майнера, вернётся сама); ваш старт (новый лист, `miner restart`, перезагрузка) даёт ей новый шанс. Защита: повторные
  сбои на карте выключают её auto-uv на 24 часа; повторные перезапуски рига во время подбора ставят его на паузу до `--uv-unpause` /
  `--uv-reset-all` или новой версии. Состояния `[AUTO-UV GPU3 OFF 24h]`, `[AUTO-UV PAUSED]`, `[AUTO-UV NOT CALM]` видны на экране майнера, в API и
  в HiveOS. MANUAL.md, раздел 30.
  Остальные параметры (`--uv-offset N`, `--uv-max N`, `--uv-reval-hours H`, `--auto-uv-dry-run`) и порядок ручной настройки —
  в [MANUAL.md](MANUAL.md#12-андервольт----auto-uv-необязательно-новое-в-140). Андервольт — на ваш риск, как и любой разгон.
- **Своего потолка смещения у майнера нет (с 1.8.1):** каждая карта поднимается, пока каждый шаг снижает мощность, самое большее до диапазона
  своего драйвера; старых пределов по типу карты больше нет. `grep range` в логе майнера показывает диапазон каждой карты. Свой предел — по
  значению на карту в порядке nvidia-smi (GPU0, GPU1, ... как в логе майнера; или в порядке `--devices`), без пробелов:
  `--auto-uv --uv-max 225,225,-,-,-,150` (`-` = без вашего предела). Список может быть короче числа карт: оставшиеся получают последнее
  значение (см. *Списки по картам* ниже).
- **Turing (RTX 20xx, GTX 16xx, CMP HX), с 1.8.1:** результат дольше подтверждается на нагрузке майнинга; карта, которая вскоре снова падает,
  сразу возвращается на стабильное смещение; профиль Turing, подобранный старой версией, один раз получает эту долгую проверку.
  MANUAL.md, раздел 30.
- **Результаты в профиль разгона:** `--hive-oc-sync` (см. ниже) сообщает подобранные offset в панель HiveOS, а с токеном записывает их в
  профиль разгона.

### Подобранные смещения в профиль разгона (`--hive-oc-sync`), с 1.8.1

С `--auto-uv`, когда карты подобраны:

- `--hive-oc-sync` шлёт **одно сообщение в панель HiveOS** (сообщения воркера): `FreeForGem auto-uv (Ting): 4 GPUs tuned at 1800 MHz:
  -44 W (-11.8 %)`, с offset, локом и ваттами до / после по каждой карте и строками `Core Clock Offset` / `Lock Core Clock` (с
  `--lock-mclock` ещё `Lock Mem Clock`), готовыми для вставки в форму разгона. Повторно — только когда результаты изменились. Ничего не записывается.
- С **API-токеном HiveOS** майнер ещё и записывает подтверждённые offset и локи ядра / памяти, которые он ставит (`--lock-cclock` /
  `--auto-lock`, `--lock-mclock`), в профиль разгона этого воркера (алгоритм листа, иначе *Default Config*); HiveOS применяет его на риге.
  Остальное в профиле (лимит мощности, вентиляторы, другие алгоритмы) не трогается; перед каждой записью сохраняется копия. С `--split` каждая группа пишет только свои карты и шлёт своё сообщение.
- **Токен:** веб HiveOS → настройки аккаунта → *Sessions* / API → новый токен **только для этого**. Впишите в *Extra config arguments*:
  `--auto-uv --hive-oc-sync --hive-oc-sync-token ВАШ_ТОКЕН`. Майнер маскирует токен на экране и в логе.
- **Или без токена в листе:** токен в полётном листе видят все, у кого есть доступ к ферме, а на риге он виден в `ps` и в конфиге рига. Если не
  хотите держать его в листе: на риге `nano /root/ffg-hive-token` (вставить, сохранить), `chmod 600 /root/ffg-hive-token`, а в
  *Extra config arguments* `--auto-uv --hive-oc-sync --hive-oc-sync-token-file /root/ffg-hive-token` (не в `/hive-config`: файлы там читает
  любой пользователь).
- `--hive-oc-sync-test` шлёт одно тестовое сообщение и выходит. Отклонённый токен (401/403): оранжевая строка, до следующего рестарта
  записей больше нет, сообщения идут дальше. MANUAL.md, раздел 32.

### Автовыбор лока ядра (по желанию, `--auto-lock`, новое в 1.7.8)

Добавьте `--auto-lock` в *Extra config arguments* (Pearl, NOID, PROX; не CMFD). Майнер переберёт локи ядра каждой карты (по умолчанию
1800 ... 1200 МГц, каждый со своим профилем `--auto-uv`, замер на настоящем майнинге: шары уходят в пул), оставит
лучший TH/W в пределах 10 % от максимального хешрейта и сохранит его в `/hive-config/freeforgem/autolock.json`: следующие старты сразу
ставят его. Первый перебор — 30-90 мин (все карты сразу); хешрейт виден в панели всё время, с короткими паузами между раундами.
Опции: `--auto-lock-range 1300-1600:100`, `--auto-lock-max-loss 10`, `--auto-lock-goal eff|speed`, `--auto-lock-reset` (перебрать заново).
Карта со своим значением `--lock-cclock` не перебирается. Подробно: MANUAL.md, раздел 23.

**По деньгам:** `--auto-lock --auto-lock-goal profit --power-price 0.12,0.053@23-07` (электричество $/кВт·ч, день и ночь 23-07 ч) оставит
лок с наибольшей прибылью в $ в сутки (доход по данным сети и цене монеты минус электричество по замеренным ваттам); каждый лок
2000 ... 1200 меряется один раз. Цена — из публичных тикеров (только GET, ничего вашего не отправляется) или `--coin-price X` (к биржам не
обращается); раз в `--auto-lock-reprice-hours` (6) выбор повторяется по замеренным точкам (переключение только при выигрыше > 3 %,
`--auto-lock-hysteresis`). `--pool-fee PCT` — для пула, которого майнер не знает. MANUAL.md, раздел 23.1.

### Требования

- HiveOS с драйвером NVIDIA 525 или новее.
- Оптимизирован под RTX 30xx и RTX 40xx (своё ядро для Ada с 1.5.4); RTX 20xx/GTX 16xx/CMP и RTX 50xx тоже поддерживаются (CMP 50HX: на драйвере 610.43.03 ~67 TH/s на карту при локе 1300 и ~77 TH/s при 1500, 166-176 Вт; на старых драйверах 550.78 / 595.91.07 только ~2,6 TH/s).
- Старые карты не поддерживаются. Карты AMD Radeon поддерживаются только в Windows (с 1.6.0); пакет для HiveOS — только NVIDIA.
- **ОЗУ:** майнингу нужно около 0,2 ГБ плюс 0,1-0,2 ГБ на карту и до ~1,5 ГБ сверх того на миг, пока он доказывает шару Pearl
  (с 1.7.3). Проверкам `--auto-uv` нужно по ~0,25 ГБ плюс ~1,6 ГБ на всех (RTX 3070 / CMP 50HX). При малом объёме свободной памяти
  майнер проверяет меньше карт за раз (в логе "checking N of M GPU(s) at a time"), а если не помещается даже одна (риг с 4 ГБ) — проверок и тюнинга нет: карта с
  проверенным профилем своих частот использует его, остальные майнят на вашем offset. Процесс проверки OOM killer Linux убивает
  первым; проверка, убитая из-за нехватки памяти, не считается провалом offset. Без `--lock-cclock` / `--lock-mclock` майнер
  ставит локи профиля OC до начала майнинга (другая программа могла оставить частоты без лока), а карты стартуют по очереди,
  через 1,5 с.
- **HiveOS перезагружает риг при высоком load average, даже с выключенным вотчдогом** (`/hive/bin/wd`, check_la: `sreboot fast`,
  когда load average за 5 мин доходит до max_la рига, 28 на риге с Xeon 4C/8T). Слабые CPU (4 ядра) с многими картами
  рискуют, пока тюнятся несколько карт сразу. С 1.7.2 майнер держит нагрузку на CPU низкой, а сводка тюнинга показывает load average с WARNING выше 2 x числа потоков CPU.

### NOID (Parano1d), с 1.7.0

Нужны NVIDIA RTX 30/40/50 и **драйвер 580+** (`nvidia-driver-update 580`). Полётный лист: монета `NOID`, кошелёк `o1...`, Miner
*Custom* `freeforgem` (та же ссылка), Hash algorithm `parano1d`, шаблон `%WAL%.%WORKER_NAME%`, Pool URL
`stratum+ssl://eu2.innovlab.cc:19601`, Extra config `--algo noid --lock-cclock 1300 --auto-uv` (режим eff, лучшие MH/Вт на RTX 30;
`--noid-mode speed --lock-cclock 1500 --auto-uv` — примерно на 31 % больше MH/s при MH/Вт примерно на 4 % ниже; таблица рига 8x RTX 3070: MANUAL.md, раздел 15). Без `--auto-uv` положительный offset ядра из OC HiveOS на время NOID
ставится в 0 (андервольт Pearl даёт тихо неверные результаты NOID) и возвращается при остановке. Подробности: MANUAL.md,
разделы 15 и 16.
На старте vardiff innovlab начинает с лёгкой цели шары, и ответы пула могут отставать на секунды; с 1.7.2 майнер ограничивает число шар,
ждущих ответа, и пропускает лишние лёгкие (строка статистики: `rej N (stale N, skipped N)`); шара, подходящая под цель блока,
отправляется всегда.

### Common Foundry (CMFD), с 1.6.0

Тот же пакет майнит CMFD (движок FreeForgeMiner, MIT, папка `cmfd/`). Полётный лист: *Hash algorithm* `forgematrix_v4`,
шаблон `%WAL%.%WORKER_NAME%`, кошелёк — адрес CMFD (`cmfd1...` или 64 hex), *Pool URL* — Aria
`cmfd+tls://159.69.194.46:29445?pin=9dfb51083f287726117f689f87bc7a878792efcca58ca8b6f6e7c05ac5d152e9` или Midas Mining
`cmfd+tls://24.107.152.151:29745?pin=2ee4e6a028b07b300753dff3c260b79763c5e748d4e78b86cf3855e97eb2e80e` (другие пулы — MANUAL.md, раздел 13).
Extra config по желанию: `--cmfd-mode eco`, `--devices 0,1`, `--per-gpu-workers`. Модель 6,4 ГБ — в `/hive/miners/custom/cmfd-model`
(общая с FreeForgeMiner, скачивается один раз). Стоковые частоты или лимит мощности: положительное смещение ядра из профиля
разгона майнер на время работы сам ставит в 0 (и возвращает); `--uv-offset N` — своё значение на ваш риск. Комиссия 1% внутри
движка (36 с в час на GPU, воркер `ffm-fee`).
**Для CMFD — отдельный профиль разгона без лока памяти.** Разгон под Pearl (память залочена на 810) режет CMFD примерно в 6 раз:
RTX 3070 с *Mem lock* 810 дала 3,7 FW/s, со свободной памятью — 21,8 FW/s (~237 Вт при лимите 240 Вт). О низкой частоте памяти
майнер предупреждает в логе (один раз на GPU, с 1.6.1), но разгон сам не меняет.
CMFD работает на образах HiveOS и с Ubuntu 20.04, и с 22.04 (движку в пакете нужна glibc 2.30+).
`--auto-uv` в *Extra config* подбирает отдельное смещение ядра под CMFD на каждой карте прямо на нагрузке CMFD (каждый шаг
проверяется побитно; первый запуск ~15–20 мин, карта при этом майнит (с 1.8.1), дальше применяется сразу; ваше смещение возвращается при
`miner stop`). Подбирайте с тем профилем разгона, с которым майните: смена лимита мощности — подбор заново, смена лока частоты
ядра — свой профиль CMFD для этого лока (подбирается один раз, с 1.7.2). MANUAL.md, раздел 13.

### Proxima (PROX), с 1.7.8

Нужна карта NVIDIA (GTX 10 / RTX 20/30/40/50). Полётный лист: монета `PROX` (если её нет в списке — добавьте как свою), кошелёк — ваш адрес
PROX (InnovLab: holder id, 64 hex), Pool *Configure in miner*, Miner *Custom* `freeforgem` (та же ссылка Installation URL).
**Hash algorithm: оставьте `----`**: в списке HiveOS нет `proxima`. Шаблон `%WAL%.%WORKER_NAME%` (соло: `%WORKER_NAME%`).

| | proximamine | InnovLab | соло |
|---|---|---|---|
| Pool URL | `stratum+ssl://pool.proximamine.com:3334` | `innov+tls://eu3.innovlab.cc:19701` | `http://IP_НОДЫ:ПОРТ` |
| Pass | secret вашего аккаунта proximamine | `x` | `mine.seeker.token` ноды |

Скриншоты всех полей: `docs/img/hiveos-prox-*.png` (также в MANUAL.md, раздел 21).
Extra config: `--auto-uv --lock-cclock 1500` (RTX 3070, RTX 3070 Laptop, RTX 3080 Ti; одно значение для всех карт или список через запятую по GPU).
Лок ядра задаёт скорость, оффсет auto-uv — ватты; рекомендуемые локи (оффсет найденный или фиксированный): RTX 3070 1500 (+120,
2,66 GH/s, 87 Вт), RTX 3070 Laptop 1500 (+270, 2,23 GH/s, 53 Вт), RTX 3080 Ti 1500 (+210, 4,40 GH/s, 147 Вт; для скорости 1800 +270, 5,28 GH/s,
201 Вт), RTX 4070 Ti **2450** (+270, 5,47 GH/s, 142 Вт). CMP 50HX не замерена (на CMP нет лока памяти). Память по умолчанию лочится на самой
низкой частоте (`--no-mem-lock` оставляет её). Разгон HiveOS из веб-интерфейса `nvidia-oc` применяет при каждом старте майнера и смене разгона, и он
сбрасывает лок и оффсет: с 1.7.8 майнер ставит их заново (в логе `... was reset by an OC apply ... set again`, не больше 4 раз на GPU в час).
На панели видны GH/s и GH/W. Пулы, опции и комиссия (1%): MANUAL.md, раздел 21.

### NEB (Neural Engine Byte), с 1.8.0

Полётный лист: монета `NEB` (если её нет в списке — добавьте как свою), кошелёк `neb1...`, Pool *Configure in miner*, Miner
*Custom* `freeforgem` (та же ссылка Installation URL):

| | Suprnova | ваш шлюз (несколько ригов на одной ноде NEB) |
|---|---|---|
| Hash algorithm | `nebhash` | `nebhash` |
| Wallet and worker template | `%WAL%.%WORKER_NAME%` | `%WAL%.%WORKER_NAME%` (блоки получает `--miner-lock` ноды) |
| Pool URL | `stratum+tcp://neb.suprnova.cc:7804` (с резервным: `stratum+tcp://neb.suprnova.cc:7804,stratum+tcp://neb.suprnova.cc:7805`) | `neb://IP_ШЛЮЗА:3442` |
| Pass | `x` | токен шлюза |
| Extra config arguments | например `--auto-uv --lock-cclock 1800 --table-mclock 810` | то же |

- Монету NEB выбирает сам URL; `--algo neb` в Extra config включает её для другого пула NEB.
- В профиле разгона можно оставить лок памяти под Pearl (810 МГц): пока майнит NEB, память работает на максимуме, а при остановке
  майнера лок профиля возвращается. Не опускайте память для NEB. `--table-mclock 810` опускает её только там, где NEB не нужна
  быстрая память (RTX 3070: −10 Вт, +8–10 % MA/W); `--tbig auto` (по умолчанию) берёт больше памяти GPU там, где это окупается.
  Модели, которой нужна полная частота памяти, она остаётся полной, и это правильно (на 810 МГц она потеряла бы большую часть
  хешрейта). С 1.8.1 такая модель не вызывает тревогу `NO TABLE`; на экране майнера `/810` бирюзовым, пока этот лок действует.
- Движок nebgpu 0.8.4 (с 1.8.1; быстрее на Turing), свой процесс движка на каждую карту: сбой CUDA на одной карте перезапускает только её (в HiveOS на каждую
  карту виден ещё один процесс `freeforgem`).
- `--power-limit N` (`--pl N`) — только по желанию и по умолчанию никогда не включается: майнер ограничивает мощность локом ядра и
  offset. `--reboot-on-lost-gpu 10` (по желанию) перезагружает риг, если карта потеряна (отвалилась от шины) 10 минут.
- HiveOS показывает khs (MA/s x 1000), скорость за 15 минут, MA/W рядом с версией, `LOCK NOT HELD`, `NO TABLE`, `GPU LOST`,
  `REBOOT NEEDED` и предупреждения NEB о памяти.
- Риг, который майнил NEB через `ffgn-hive`, сохраняет подобранные offset (`/hive-config/ffgn/uv.json` подхватывается при первом
  старте).
- Весь риг на NEB — этот полётный лист; NEB на части карт, а на остальных другая монета — *Разные монеты на разных картах* ниже.
  Подробно: MANUAL.md, раздел 24.

### Ting / Daler (DLR), с 1.8.0

Майнер Ting FFGT с 1.8.0 входит в FreeForGem (те же ядра, те же цифры). NVIDIA GTX 10 и новее (CUDA), AMD (OpenCL).
Полётный лист: монета `DLR` (добавьте как свою), кошелёк — ваш аккаунт Ting (64 hex, **без** `&net=...`; приложение Ting ID или
`ting wallet account`), Pool *Configure in miner*, Miner *Custom* `freeforgem` (та же ссылка Installation URL).
**Hash algorithm: оставьте `----`** (в списке HiveOS нет `ting-sha256`). Шаблон `%WAL%.%WORKER_NAME%`.

| | crypto-connection.net | ваш ffgt-gate | соло на ноде этого рига |
|---|---|---|---|
| Pool URL | `crypto-connection.net:4073` | `ting://IP_ШЛЮЗА:ПОРТ` | `node://127.0.0.1:18440` |
| Pass | `x` | пароль рига на шлюзе | — (Extra config `--node-dir DIR`) |

Extra config: `--lock-cclock 1800 --auto-uv` (RTX 3070: 1500 — лучшие GH/W, 1800 — больше GH/s). Память по умолчанию лочится на самой низкой частоте (SHA-256 её не использует: около −10 % мощности; `--no-mem-lock`
оставляет её; лок памяти из OC-профиля HiveOS сохраняется; у CMP 50HX драйвер не даёт лочить память). Групповой перебор (по умолчанию) даёт
около +40 % в соло и +25 % на пуле на RTX 30 (`--no-group` выключает). На панели —
GH/s и GH/W, algo `ting`. Каждая карта майнит в своём процессе движка: сбой CUDA на одной карте перезапускает только её за несколько
секунд (в HiveOS на каждую карту виден ещё один процесс `freeforgem`; `--single-process` — старый режим). Комиссия 1 % (60 с из 100 мин)
на crypto-connection.net. Опции: `freeforgem --algo ting --help`. Подробно: MANUAL.md, раздел 26.

### Разные монеты на разных картах (`--split`), с 1.8.0

Один полётный лист на весь риг: **основная монета** задаётся в листе как обычно (её Pool URL, Wallet и Pass), **вторая монета**
пишется в *Extra config arguments* значениями с тегом её имени (`--pool pearl=...`, `--wallet pearl=...`). Значение с тегом заменяет
для группы этой монеты значение из листа без тега, поэтому пул и кошелёк листа остаются основной монете.

**Пример: GPU0 и GPU1 майнят NEB на Suprnova, GPU2 и GPU3 — Pearl на Kryptex** (кошельки-заглушки замените своими):

1. **Кошельки.** Кошелёк NEB (`neb1...`). Адрес PRL пишется в Extra config, отдельный кошелёк в HiveOS для него не нужен.
2. **Полётный лист:** монета `NEB`, кошелёк NEB, Pool *Configure in miner*, Miner *Custom* → *Setup Miner Config*:

   | Поле | Значение |
   |---|---|
   | Miner name | `freeforgem` |
   | Installation URL | ссылка выше (1.8.1) |
   | Hash algorithm | `nebhash` |
   | Wallet and worker template | `%WAL%.%WORKER_NAME%` |
   | Pool URL | `stratum+tcp://neb.suprnova.cc:7804` |
   | Pass | `x` |
   | Extra config arguments | `--split neb:0,1 pearl:2,3 --pool pearl=prl-eu.kryptex.network:7048 --wallet pearl=prl1ВАШ_АДРЕС_PRL --lock-cclock neb=1800 --lock-cclock pearl=1410 --table-mclock neb=810 --auto-uv` |

3. **Разгон:** профиль может лочить память всех карт на 810: карты NEB на время майнинга получат полную частоту памяти (и 810
   обратно при выходе), карты Pearl её сохранят. Локи ядра задают `--lock-cclock neb=...` / `pearl=...`.
4. **Примените** лист к ригу.

Правила для Extra config:

- **Группы:** `МОНЕТА:КАРТЫ` с номерами карт как на панели (`neb:0,1 pearl:2,3`, диапазоны `neb:0-3`, одна группа может взять
  `rest`: `neb:0,1 pearl:rest`). Пробел, `,` и `;` — равноценные разделители, кавычки не нужны. Одна карта майнит одну монету.
- **Опции без тега идут во все группы** (`--auto-uv`, воркер из шаблона); **с тегом** — только в группу своей монеты
  (`--lock-cclock pearl=1410`). Опции, которые есть только у некоторых монет (у NEB `--table-mclock`, `--pl`, `--tbig`; у Ting
  `--no-group`; ...), сами уходят только в группы этих монет; в логе майнера на каждую группу есть строка `split:` о том, что ей не
  передано.
- **Список по картам без тега — на весь риг**, он нарезается по группам: `--lock-cclock 1800,1800,1410,1410` даёт NEB 1800,1800, а
  Pearl 1410,1410.
- **HiveOS режет Extra config по пробелам**, поэтому `--split-args` берёт одно слово: пишите `--split-args neb=--auto-uv` отдельно
  для каждого флага одной группы, а опции со значением — тегом (`--table-mclock neb=810`, а не `--split-args neb="--table-mclock 810"`).
- Pool URL, Wallet и Pass листа (и соло-ноду) скрипты HiveOS майнера помечают тегом монеты листа, поэтому они уходят только в группу
  этой монеты: пароль шлюза никогда не попадёт на пул другой монеты.
- Значение с тегом (`--table-mclock neb=810`) — чтобы указать явно или дать двум группам разные значения.

Что показывает панель:

- **algo и общий хешрейт** группы, в которой больше карт (из групп с одинаковым числом карт — первой в `--split`);
- **каждую карту** с её собственной скоростью; если у монет разные единицы, каждая карта отдаётся в своих «штуках в секунду» (NEB —
  попытки/с, Pearl — хеши/с), так что сравнивать эти числа между монетами нельзя;
- **принятые / отклонённые** шары всех групп вместе;
- **колонка версии** — каждая группа со своей эффективностью: `1.8.1 split: neb 1.40 MA/W + pearl 0.61 TH/W`, плюс метки вроде
  `LOCK NOT HELD` или `GPU LOST` той группы, к которой они относятся.

Каждая группа работает своим процессом майнера со своей сессией пула, комиссией, частотами, политикой памяти, тюнерами и
перезапусками. Завершившаяся группа сама запускается снова через 10 с (после 3 завершений за 10 мин — пауза 10 мин), остальные
группы майнят дальше. `miner stop` останавливает все группы, и каждая возвращает свои частоты. В логе майнера
(`/var/log/miner/freeforgem/freeforgem.log`) каждая строка группы начинается с `[neb]` / `[pearl]` (голубым на экране майнера),
собственные строки супервизора — со `split:`. Проверено на HiveOS-риге из 4 карт (2 NEB + 2 Pearl). Подробно: MANUAL.md, раздел 27.

### Списки по картам любой длины, с 1.8.0

Каждая опция по картам (`--lock-cclock`, `--lock-mclock`, `--moffset`, `--uv-offset`, `--uv-max`, `--uv-min`, `--temp-limit`,
`--temp-stop`, у NEB — `--table-mclock` и `--power-limit`) принимает список любой длины, как списки разгона HiveOS: короткий список
отдаёт последнее значение оставшимся картам, значения после последней карты игнорируются, `-` / пустое поле / `0` там, где ноль
недопустим, — карта не трогается. Поэтому один лист подходит ригам с разным числом карт: на риге из 8 карт
`--table-mclock 810,810,-,-` — 810 на GPU0 и GPU1, остальные шесть не трогаются (до 1.8.0: «4 values for 8 device(s)», и майнер не
стартовал). MANUAL.md, раздел 28.

### Экран майнера: таблица карт, с 1.8.1

Откройте Hive Shell (или SSH) и наберите `miner`: вместо бегущего лога майнер показывает таблицу карт: хешрейт, температура,
вентилятор в %, ватты, лимит мощности, на ватт, частоты и локи, смещение V/F и шары каждой карты, итог рига, а сверху — пул и
воркер, `net` (блок, задание, пинг, stale), `mode` (алгоритм, auto-uv, локи), ваши параметры (`args`, секреты скрыты), последние
важные события (`!`, постоянный блок из 5 строк) и живые тревоги (`!!`). Ячейка OFFSET: оранжевая `► +300 ↑ 3/9`, пока идёт подбор,
оранжевая `✗ -2` / `× roll-back` — сбой шага и откат, зелёная `✓ -9.6%` — подобрано, жёлтый `-` — auto-uv выключен; `OUT 14m` — карта
исключена после повторных сбоев, вернётся сама. Оранжевое — подбор делает свою работу, красное требует ваших рук. Смысл каждой
колонки, значка и цвета: MANUAL.md, раздел 31.

- `miner log`, «Miner log» в веб-интерфейсе и h-stats.sh читают лог как раньше: все строки на месте, без изменений.
- h-run.sh переводит окно майнера в UTF-8 и выключает журнал screen для этого окна, поэтому в `/run/hive/miner.N` идут строки лога, а
  не перерисовка экрана. В GNU screen 16 цветов: оранжевый выглядит тусклым жёлтым (коричнево-оранжевым в веб-Hive Shell), ✗ ✓ — × √.
- Параметры в *Extra config arguments*: `--ui lines` — прежний бегущий лог; `--ui ascii` — если рамка выглядит сломанной (странные
  буквы вместо линий); `--no-color` — без цвета; `--ui-demo split` в консоли показывает таблицу с выдуманным ригом.
- BAD — неверные результаты карты **при майнинге**, пойманные CPU-проверкой (в пул не ушли; это не плохие шары — отклонённые пулом
  в REJ). Неверные результаты на шаге подбора auto-uv — работа защиты: их показывает ячейка OFFSET на время шага (`+390 ✗ 15 wrong`),
  в BAD они не идут.

**Окно не закрывается (с 1.8.1).** После выхода с ошибкой h-run.sh пишет `freeforgem exited (code N), restarting in 5 s`
(экран, лог, `miner log`) и запускает майнер снова в том же окне; одноразовые флаги (`--uv-reset`, `--uv-reset-all`,
`--uv-unpause`, `--uv-parallel-reset`, `--auto-lock-reset`) на таком старте пропускаются. `miner stop` / `miner restart` (и
watchdog HiveOS) заканчивают цикл, как раньше. После частых выходов окно ждёт 10 минут перед следующим стартом;
`miner restart` запускает сразу. Окно закрывается («[screen is
terminating]») только при `miner stop` / `miner restart`.

### Сбои и потерянные карты, с 1.8.0

- После падения auto-uv продолжает с упавшего offset **минус 2 шага** (все монеты).
- После сбоя карта сначала переводится в **безопасное состояние** (в том числе безопасный offset), и только потом её движок
  стартует снова.
- **FAULTY:** 3 сбоя за 10 минут на безопасном offset (NEB, Ting) — карта отдыхает 10 минут, затем получает новый движок. Она не
  потеряна, перезагрузка не нужна: проверьте её частоты, питание и райзер.
- **DEAD / LOST:** только карта, которая больше не открывается, Xid 79 (отвалилась от шины) или карта, которую больше не видит
  NVML: на панели она считается нулём (а не последней скоростью, поэтому вотчдог HiveOS видит настоящий итог), а в колонке версии —
  `GPU LOST` / `REBOOT NEEDED`. По желанию `--reboot-on-lost-gpu 10` (NEB) перезагружает риг через 10 минут такого состояния (не в
  первые 10 минут работы майнера и не чаще раза в час).
- **Устаревшие шары:** задание, на которое пул ответил «job not found», сразу гасится; если ответы пула приходят с опозданием в
  секунды, лог пишет `WARNING: the pool answers late` (теряются пакеты в сети рига: проверьте сетевую карту, кабель и свитч).

MANUAL.md, раздел 29.

### Карты CMP: разблокированный драйвер (по желанию, по умолчанию выключено, 1.7.7)

Под штатным драйвером NVIDIA карта CMP (CMP 50HX и семейство) выдаёт около десятой части скорости. Чтобы майнер поставил разблокированный драйвер,
добавьте слово `cmp-unlock-driver` в *Extra config arguments* (майнер его не получает). Перед майнингом он проверяет, ставит драйвер, только если
разблокированной сборки ещё нет, и **перезагружает риг**; риг, у которого она уже стоит, без карты CMP или без root, не трогается.
**Это заменяет драйвер ядра NVIDIA на всём риге:** сначала прочитайте MANUAL.md, раздел 20. Вручную: `sudo ./freeforgem --cmp-driver`
(`FFG_CMP_DRIVER_DRY_RUN=1` или `--cmp-driver --dry-run` только сообщает, что было бы сделано; риг, разблокированный rgminer, не трогается).

### Резервные пулы (failover и failback), с 1.7.8

В поле **Pool URL** полётного листа можно указать несколько URL одной монеты через пробел или запятую; первый — основной:

    pearl-eu2.luckypool.io:3360 prl-eu.kryptex.network:7048

Если пул недоступен, отказал в логине или не прислал задание, майнер переходит к следующему URL; пока он на резервном, основной
проверяется раз в 10 минут отдельным соединением (login + задание; `--failback-minutes N` в *Extra config arguments*, `0` — выкл.), и после
2 удачных проверок подряд майнер возвращается на основной, не теряя шары «в полёте». Комиссия следует за вами на тот пул, где вы сейчас.
CMFD: движки перезапускаются на следующем пуле (около минуты без майнинга на каждое переключение). `--pool-check` в консоли
(`./freeforgem --pool "A,B" --wallet W --pool-check`) проверяет каждую запись без GPU. Подробнее: MANUAL.md, раздел 22. Соло-URL
`http://` — одна нода, списка нет.
`--noshare-minutes M` (15) и `--reject-minutes K` (5) в *Extra config arguments* (0 — выкл.): пул, который принимает соединение, но не
засчитывает ваши шары (при работающих GPU и ожидаемых шарах), переподключается, затем берётся следующий пул списка.
С 1.8.1 испорченная сессия пула заменяется новой (NEB, NOID, PROX, Ting, stratum Pearl): если пул уронил сложность шары или отвечает
поздно при большом числе stale. В логе и на экране майнера — `pool: reconnecting: <причина>` (жёлтым).

### Комиссия разработчика

1%: 60 секунд из каждых 100 минут, на том же пуле, что указан у вас (в 1.8.1 без изменений). Подробнее — в [LICENSE](LICENSE).
С 1.8.0 комиссия каждой монеты идёт по своему соединению: NEB — на вашем списке пулов (соло / шлюз: публичные пулы NEB), Ting — на
crypto-connection.net, PROX — второй логин на вашем пуле (proximamine только запасной), CMFD — внутри движка (36 с в час на карту). С `--split` каждая группа платит 1 % времени
своих карт. Если соединение к слоту не готово, слот повторяется до 3 раз.

## 中文

FreeForGem 在 HiveOS 中作为 **Custom miner（自定义矿工）** 运行：无需 SSH，也无需手动安装。

**从 FreeForGemPear 迁移：** 自 1.7.0 起，多币种矿工名为 **FreeForGem**（自定义矿工 `freeforgem`）。HiveOS 把它视为另一个矿工：
**请新建一个飞行表**，Miner name 为 `freeforgem`，并使用下面的链接（旧的 `freeforgempear` 飞行表会继续运行旧矿工）。已保存的 `--auto-uv`
结果在首次启动时自动接管（从 `/hive-config/freeforgempear/uv.json`）。FreeForGemPear 本身继续作为 Pearl 矿工。

**安装链接（1.8.1）：**

```
https://github.com/pepsykolya/FreeForGem/releases/download/v1.8.3/freeforgem-1.8.3_hiveos.tar.gz
```

HiveOS 按文件名缓存下载的压缩包：每个版本的文件名都不同，所以请始终粘贴新版本的链接。
你自己构建或改名的软件包，文件名中必须**只有一个短横线**（`freeforgem-1.8.3_hiveos.tar.gz`，名称-版本_hiveos）：
如果有第二个短横线，HiveOS 会取错矿工名称，矿工会静默地不启动。同一文件名下的新构建不会被重新下载：请把它放到 URL 的新目录中，
或在矿机上运行 `/hive/miners/custom/custom-get <URL> -f`。

### 飞行表

1. **钱包。** 在 *Wallets* 中添加你的 PRL 钱包：币种 `PRL`，地址 `prl1...`。在 LuckyPool 上还可以合并挖 NOCK：
   地址写成 `prl1...+你的NOCK地址`（仅 LuckyPool；见 MANUAL.md 第 6 节）。
2. **新建飞行表。**
   - **Coin：** `PRL`。
   - **Wallet：** 第 1 步中的钱包。
   - **Pool：** *Configure in miner*。
   - **Miner：** *Custom*，然后点击 *Setup Miner Config*。
3. **矿工配置：**

   | 字段 | 值 |
   |---|---|
   | Miner name | `freeforgem`（粘贴 URL 后自动填写） |
   | Installation URL | 上面的链接 |
   | Hash algorithm | `pearlhash`（或保留 `----`） |
   | Wallet and worker template | `%WAL%.%WORKER_NAME%` |
   | Pool URL | `pearl-eu2.luckypool.io:3360`，或 Kryptex `prl-eu.kryptex.network:7048`（或任何其他 Pearl stratum 矿池） |
   | | Kryptex：钱包也可以是你的 Kryptex 用户名（以 BTC/USDT 支付并自动兑换，自 1.7.4 起） |
   | Pass | 留空 |
   | Extra config arguments | 可选，例如 `--devices 0,1`；`--auto-uv` 开启降压调校（见下文） |

4. **应用。** *Apply changes*、*Create flight sheet*，然后把它应用到矿机。

矿工日志位于 `/var/log/miner/freeforgem/freeforgem.log`，也可在 HiveOS 矿工控制台中查看。
面板显示每块显卡和总的算力、温度、风扇，以及接受和拒绝的份额；自 1.7.2 起在矿工版本旁显示矿机效率（Pearl 为 TH/W，NOID 为 MH/W）；
自 1.7.5 起还显示是否有新版本（`1.7.8 0.652 TH/W, need update!!`；`--no-update-check` 关闭检查）。

### 超频（RTX 3070）

Pearl 不需要高显存频率：

| 设置 | 值 |
|---|---|
| 核心频率 | 绝对值，`1560` MHz |
| 显存频率 | 绝对值，`810` MHz（HiveOS 中：Memory clock `810`，或你的显卡接受的最低值） |
| 功耗限制 | `120` W |

- **1560 MHz：** 约 70 TH/s，约 107 W。

### 降压（可选，`--auto-uv`，1.4.0 新增）

默认关闭。在 *Extra config arguments* 中加入 `--auto-uv`（例如 `--devices 0,1 --auto-uv`），矿工会为每块 GPU 一次性找到显卡仍能正确计算的
最低核心电压（V/F 偏移），挖矿时应用，停止时恢复你自己的设置。算力不变，功耗更低（RTX 3070 在 1560 MHz：每卡约少 5-10 W）。

- 需要 root 权限，HiveOS 的矿工本来就有。没有 NVML 或权限时，它会在日志中写明原因并正常挖矿。
- **首次调校只需几分钟**（自 1.8.1 起所有 GPU 同时调校；4x RTX 3070 矿机：NEB 1.5-2 分钟，PROX 约 5，Ting 6-8.5，NOID 约 9，CMFD 10-18）。
  在此期间矿机在所有币种上继续挖矿，正在调校的 GPU 也一样：只有在检查某一步时算力略低，其份额照常提交到矿池。结果保存在
  `/hive-config/freeforgem/uv.json`（矿工更新和重启后仍保留），**每个核心锁频一个配置**（自 1.7.2 起）：在超频配置中切换锁频会复用该锁频
  已验证的结果，新锁频只调校一次；更换驱动或内核构建时会重新调校。挖矿中崩溃的 GPU 会在日志中点名，并由矿工自行降低偏移（`gpu-faults.json`；
  Linux / HiveOS：在同一进程中自行重启，每小时最多 12 次），见 MANUAL.md 第 17 节。
- 先设置好 Overclock 选项卡（核心锁频 `1560`、显存、功耗限制）。调校从超频配置的偏移开始（RTX 3070：从对应核心锁频的默认值开始）；
  启动时，其他矿工留在显卡上的偏移会被超频配置的值替换；它从不改动锁定的频率、功耗限制或显存。当 HiveOS 在挖矿时重新应用超频（在矿工启动时
  或超频更改后，它会写入 *Core Clock* 的值，**字段为空时写 0**），auto-uv 会在快速检查后把已验证的偏移放回去（自 1.7.1 起；之前降压会关闭
  直到下次重启）。手动设置了不同的偏移，则 auto-uv 在该 GPU 上让步。
- *Extra config* 中的 `--lock-cclock` / `--lock-mclock`：退出时矿工会恢复你 HiveOS 超频配置中的锁频（自 1.7.1 起；之前会移除锁频，
  矿机在未锁频状态下运行）。
- 只改动矿工正在使用的 GPU（按 PCI 总线 id 匹配，日志中可见）。每次改动都记录为
  `auto-uv: GPU0 (bus 0d, RTX 3070) offset +150 -> +195, 116.6 W -> 106.6 W, saved`；在矿工日志中 `grep auto-uv` 可查看全部。
- 停止矿工会恢复你的偏移。如果矿工被强制结束，下次启动时首先恢复它。
- 调校从接近预期结果的位置开始，因此很短；`--uv-min N` 设置起始偏移。矿工屏幕显示每块 GPU 的进度
  和颜色（`--no-color` 关闭颜色）。
- **重置：** 加上 `--uv-reset` 启动一次：它只清除当前锁频的配置（`--uv-reset-all`：清除所有配置；或删除 `/hive-config/freeforgem/uv.json`）。
  退出时会恢复超频配置中的偏移（*Core Clock*）。
  自 1.8.1 起，`--uv-reset`、`--uv-reset-all`、`--uv-unpause` / `--uv-parallel-reset` 和 `--auto-lock-reset` **只在你主动进行的启动时生效一次**
  （新的飞行表、`miner restart`、正常停止后重启机器）；故障后的重启会跳过它们。那次启动之后请把它们从飞行表中删除，并且**绝不要把
  `--uv-unpause` 留在飞行表中**：它会在每次启动时解除重启保护。
- **调校期间继续挖矿和保护机制，自 1.8.1 起（所有币种）。** 1.8.1 之前，每个调校步骤都会停止该卡的挖矿（Pearl：最长约 14 分钟没有算力，
  矿池看不到该矿工）；现在 Pearl 的预热也在挖矿。所有 GPU 同时调校，数量以矿机空闲内存允许为限；整台矿机出问题后（GPU 从总线掉线、检查卡死）
  改为每次 1 块，并为该矿机记住（`--uv-parallel N` 手动设置，`--uv-parallel-reset` 清除谨慎状态）。失败的步骤在失败偏移以下 2 步处继续；
  挖矿中崩溃时，显卡直接在崩溃偏移以下 2 步处重新开始，无需从头调校。故障按币种计数：反复崩溃的显卡会暂时被排除（矿工屏幕中显示 `OUT 14m`，
  会自行回来）；你主动进行的启动（新的飞行表、`miner restart`、重启机器）会给它新的机会。保护：同一块卡反复失败，该卡的 auto-uv 关闭
  24 小时；调校期间矿机反复重启，调校暂停，直到 `--uv-unpause` / `--uv-reset-all` 或新版本。状态 `[AUTO-UV GPU3 OFF 24h]`、
  `[AUTO-UV PAUSED]`、`[AUTO-UV NOT CALM]` 显示在矿工屏幕、API 和 HiveOS 中。MANUAL.md 第 30 节。
  其他选项（`--uv-offset N`、`--uv-max N`、`--uv-reval-hours H`、`--auto-uv-dry-run`）以及手动调校步骤见 MANUAL.md 第 12 节。
  降压和任何超频一样，风险自负。
- **矿工没有自己的偏移上限（自 1.8.1 起）：** 只要每一步都能降低功耗，每块卡就继续爬升，最多到其驱动的范围；以前按显卡类型的上限已取消。
  在矿工日志中 `grep range` 可查看每块 GPU 的范围。如需设置自己的上限，按 nvidia-smi 顺序（GPU0、GPU1……与矿工日志一致；或按 `--devices`
  顺序）每块 GPU 给一个值，不加空格：`--auto-uv --uv-max 225,225,-,-,-,150`（`-` = 不设你的上限）。列表可以比 GPU 数量短：最后一个值用于
  其余的 GPU（见下文 *任意长度的按卡列表*）。
- **Turing（RTX 20xx、GTX 16xx、CMP HX），自 1.8.1 起：**结果在挖矿负载上确认更长时间；不久后再次崩溃的显卡立即回到稳定偏移；
  旧版本调校的 Turing 配置会进行一次这种较长的检查。MANUAL.md 第 30 节。
- **结果写入超频配置：** `--hive-oc-sync`（见下文）把调好的偏移报告到 HiveOS 面板，有令牌时还会写入超频配置。

### 把调好的偏移写入超频配置（`--hive-oc-sync`），自 1.8.1 起

使用 `--auto-uv` 时，显卡调好之后：

- `--hive-oc-sync` 向 **HiveOS 面板发送一条消息**（该矿机的消息列表）：`FreeForGem auto-uv (Ting): 4 GPUs tuned at 1800 MHz:
  -44 W (-11.8 %)`，包含每块卡的偏移、锁频和调校前后的功耗，以及可直接粘贴到超频表单的 `Core Clock Offset` / `Lock Core Clock` 行
  （使用 `--lock-mclock` 时还有 `Lock Mem Clock`）。只有结果变化时才会再次发送。不写入任何内容。
- 有 **HiveOS API 令牌** 时，矿工还会把已确认的偏移以及它设置的核心 / 显存锁频（`--lock-cclock` / `--auto-lock`、`--lock-mclock`）写入此矿机
  的超频配置（飞行表的算法，否则为 *Default Config*）；HiveOS 在矿机上应用它。配置的其余部分（功耗限制、风扇、其他算法）不会改动；每次写入前
  都会保存副本。使用 `--split` 时，每组只写入自己的显卡并发送自己的消息。
- **令牌：** HiveOS 网页 → 账户设置 → *Sessions* / API → 创建一个**专门用于此的**新令牌。写进 *Extra config arguments*：
  `--auto-uv --hive-oc-sync --hive-oc-sync-token 你的令牌`。矿工在屏幕和日志中会隐藏令牌。
- **或者不把令牌放在飞行表中：**飞行表中的令牌，所有能访问该矿场的人都能看到，在矿机上它也出现在 `ps` 和矿机配置中。如果不想放在飞行表里：
  在矿机上 `nano /root/ffg-hive-token`（粘贴、保存），`chmod 600 /root/ffg-hive-token`，并在 *Extra config arguments* 中写
  `--auto-uv --hive-oc-sync --hive-oc-sync-token-file /root/ffg-hive-token`（不要放在 `/hive-config`：那里的文件所有用户都能读取）。
- `--hive-oc-sync-test` 发送一条测试消息后退出。令牌被拒绝（401/403）：一行橙色提示，直到下次重启不再写入，消息继续发送。MANUAL.md 第 32 节。

### 自动选择核心锁频（可选，`--auto-lock`，1.7.8 新增）

在 *Extra config arguments* 中加入 `--auto-lock`（Pearl、NOID、PROX；不支持 CMFD）。矿工会遍历每块 GPU 的核心锁频（默认 1800 ... 1200 MHz，
每个锁频有自己的 `--auto-uv` 配置，在真实挖矿中测量：份额照常提交到矿池），在最高算力的 10 % 以内保留 TH/W 最好的锁频，并保存在
`/hive-config/freeforgem/autolock.json`：之后的启动直接应用。首次遍历需要 30-90 分钟（所有 GPU 同时进行）；面板上始终显示算力，各轮之间有
短暂间隙。选项：`--auto-lock-range 1300-1600:100`、`--auto-lock-max-loss 10`、`--auto-lock-goal eff|speed`、`--auto-lock-reset`（重新遍历）。
设置了自己 `--lock-cclock` 值的 GPU 不参与遍历。详见 MANUAL.md 第 23 节。

**按收益：** `--auto-lock --auto-lock-goal profit --power-price 0.12,0.053@23-07`（电价 $/kWh，白天和夜间 23-07 点）保留每天美元收益最高的锁频
（根据网络数据和币价计算的收入减去按实测功耗计算的电费）；2000 ... 1200 的每个锁频各测一次。价格来自公开行情（仅 GET，不发送你的任何信息）
或 `--coin-price X`（不询问交易所）；每隔 `--auto-lock-reprice-hours`（6）根据已测点重新选择（只有收益多出 3 % 以上才切换，
`--auto-lock-hysteresis`）。矿工不认识的矿池用 `--pool-fee PCT`。MANUAL.md 第 23.1 节。

### 系统要求

- HiveOS，NVIDIA 驱动 525 或更新。
- 针对 RTX 30xx 和 RTX 40xx 优化（自 1.5.4 起有自己的 Ada 内核）；也支持 RTX 20xx/GTX 16xx/CMP 和 RTX 50xx（CMP 50HX：驱动 610.43.03 下锁频
  1300 每卡约 67 TH/s，1500 时约 77 TH/s，166-176 W；旧驱动 550.78 / 595.91.07 下只有约 2.6 TH/s）。
- 不支持更老的 GPU。AMD Radeon 显卡仅在 Windows 上支持（自 1.6.0 起）；HiveOS 软件包仅支持 NVIDIA。
- **内存：** 挖矿需要约 0.2 GB，每块 GPU 再加 0.1-0.2 GB，证明 Pearl 份额时会短暂多用约 1.5 GB（自 1.7.3 起）。`--auto-uv` 检查每个约需
  0.25 GB，另外所有检查共用约 1.6 GB（RTX 3070 / CMP 50HX）。空闲内存少时，矿工同时检查的 GPU 更少（日志："checking N of M GPU(s) at a time"），
  连一个都放不下时（4 GB 的矿机）则不做检查和调校：有该频率已验证配置的 GPU 使用它，其他 GPU 在你的偏移上挖矿。检查进程是 Linux OOM killer
  首先结束的进程；因内存不足被结束的检查不算作偏移失败。没有 `--lock-cclock` / `--lock-mclock` 时，矿工在挖矿前设置超频配置中的锁频（其他程序
  可能让频率处于未锁定状态），GPU 逐个启动，间隔 1.5 秒。
- **即使关闭看门狗，HiveOS 也会在负载平均值过高时重启矿机**（`/hive/bin/wd`，check_la：5 分钟负载平均值达到矿机的 max_la 时 `sreboot fast`，
  4C/8T Xeon 矿机上为 28）。CPU 较弱（4 核）而显卡较多的矿机，在多块 GPU 同时调校时有风险。自 1.7.2 起矿工保持较低的 CPU 负载，调校摘要会显示
  负载平均值，超过 CPU 线程数 2 倍时给出 WARNING。

### CMP 显卡：解锁驱动（可选，默认关闭，1.7.7）

在 NVIDIA 原厂驱动下，CMP 显卡（CMP 50HX 及同系列）只能发挥约十分之一的速度。要让矿工安装解锁驱动，请在 *Extra config arguments* 中加入
单词 `cmp-unlock-driver`（矿工本身不会收到它）。挖矿前它会检查，只有在尚未安装解锁版本时才安装驱动，并**重启矿机**；已经安装、没有 CMP 显卡
或没有 root 权限的矿机不受影响。**这会替换整台矿机的 NVIDIA 内核驱动：** 请先阅读 MANUAL.md 第 20 节。手动：`sudo ./freeforgem --cmp-driver`
（`FFG_CMP_DRIVER_DRY_RUN=1` 或 `--cmp-driver --dry-run` 只报告将要做什么；已被 rgminer 解锁的矿机不受影响）。

### 备用矿池（故障切换与自动回切），自 1.7.8 起

飞行表的 **Pool URL** 字段可以填写同一币种的多个 URL，用空格或逗号分隔；第一个为主矿池：

    pearl-eu2.luckypool.io:3360 prl-eu.kryptex.network:7048

如果某个矿池无法连接、拒绝登录或不发送任务，矿工会转到下一个 URL；在备用矿池上时，它每 10 分钟用单独的连接检查一次主矿池（登录 + 一个任务；
*Extra config arguments* 中 `--failback-minutes N`，`0` = 关闭），连续 2 次检查良好后切回，不丢失传输中的份额。开发者抽水跟随你所在的矿池。
CMFD：引擎在下一个矿池上重启（每次切换约一分钟不挖矿）。在 shell 中用 `--pool-check`（`./freeforgem --pool "A,B" --wallet W --pool-check`）
可在没有 GPU 的情况下测试每个条目。详见 MANUAL.md 第 22 节。`http://` 单独挖矿 URL 是一个节点：不支持列表。
*Extra config arguments* 中的 `--noshare-minutes M`（15）和 `--reject-minutes K`（5）（0 = 关闭）：矿池接受连接但不记入你的份额（GPU 在算、
本应有份额）时会重新连接，然后使用列表中的下一个矿池。
自 1.8.1 起，变坏的矿池会话会被新会话替换（NEB、NOID、PROX、Ting、Pearl stratum）：矿池降低了份额难度，或回答很晚且过期份额很多时。
日志和矿工屏幕显示 `pool: reconnecting: <why>`（黄色）。

### 开发者抽水

1%：每 100 分钟中的 60 秒，在你设置的同一矿池上（1.8.1 未改变）。见 [LICENSE](LICENSE)。
自 1.8.0 起每种币的抽水都走自己的连接：NEB 在你的矿池列表上（单独挖矿 / 网关：公共 NEB 矿池），Ting 在 crypto-connection.net 上，PROX 在你的
矿池上第二次登录（proximamine 仅作备选），CMFD 在引擎内部（每块 GPU 每小时 36 秒）。使用 `--split` 时每组按自己显卡的时间支付 1 %。
连接未就绪的抽水时段最多重试 3 次。

### NOID（Parano1d），自 1.7.0 起

需要 NVIDIA RTX 30/40/50 和 **驱动 580+**（`nvidia-driver-update 580`）。飞行表：币种 `NOID`，钱包 `o1...`，Miner *Custom* `freeforgem`
（同一安装链接），Hash algorithm `parano1d`，模板 `%WAL%.%WORKER_NAME%`，Pool URL `stratum+ssl://eu2.innovlab.cc:19601`，Extra config
`--algo noid --lock-cclock 1300 --auto-uv`（eff 模式，RTX 30 上 MH/W 最佳；`--noid-mode speed --lock-cclock 1500 --auto-uv` 可多约 31 % MH/s，
MH/W 约低 4 %；8x RTX 3070 的表格：MANUAL.md 第 15 节）。没有 `--auto-uv` 时，HiveOS 中正的核心偏移在 NOID 期间被设为 0（Pearl 的降压会让
NOID 静默地算错），矿工停止时恢复。结果：MANUAL.md 第 15 和 16 节。
启动时 innovlab 的 vardiff 从容易的份额目标开始，矿池的回复可能延迟数秒；自 1.7.2 起矿工限制等待回复的份额数量，并跳过多余的容易份额
（统计行：`rej N (stale N, skipped N)`）；区块份额总是会发送。

### Common Foundry（CMFD），自 1.6.0 起

同一软件包也挖 CMFD（引擎：FreeForgeMiner，MIT，位于 `cmfd/`）。飞行表：*Hash algorithm* `forgematrix_v4`，*Wallet and worker template*
`%WAL%.%WORKER_NAME%`，钱包为你的 CMFD 地址（`cmfd1...` 或 64 位十六进制），*Pool URL* Aria
`cmfd+tls://159.69.194.46:29445?pin=9dfb51083f287726117f689f87bc7a878792efcca58ca8b6f6e7c05ac5d152e9` 或
Midas Mining `cmfd+tls://24.107.152.151:29745?pin=2ee4e6a028b07b300753dff3c260b79763c5e748d4e78b86cf3855e97eb2e80e`
（其他矿池：MANUAL.md 第 13 节）。可选 *Extra config*：`--cmfd-mode eco`、`--devices 0,1`、`--per-gpu-workers`。
6.4 GB 的模型放在 `/hive/miners/custom/cmfd-model`（与 FreeForgeMiner 共用；只下载一次）。
原厂频率或功耗限制：超频配置中正的核心偏移在运行期间由矿工设为 0（并在之后恢复）；`--uv-offset N` 强制使用某个值，风险自负。开发者抽水 1%
在引擎内部（每块 GPU 每小时 36 秒，矿工名 `ffm-fee`）。
**CMFD 请使用单独的、不锁显存的超频配置。** Pearl 的超频（显存锁定在 810）会让 CMFD 慢约 6 倍：一块 RTX 3070 在 *Mem lock* 810 时为
3.7 FW/s，显存不锁时为 21.8 FW/s（约 237 W，功耗限制 240 W）。矿工会在日志中提示显存频率过低（每块 GPU 一次，自 1.6.1 起），但不会改动你的超频。
CMFD 可在基于 Ubuntu 20.04 和 22.04 的 HiveOS 镜像上运行（软件包中的引擎需要 glibc 2.30+）。
*Extra config* 中的 `--auto-uv` 在 CMFD 负载上为每块卡单独调校 CMFD 核心偏移（每一步逐位检查；首次启动约 15–20 分钟，期间显卡继续挖矿
（自 1.8.1 起），之后直接应用；`miner stop` 时恢复你的偏移）。请用你挖矿时使用的超频配置调校：改变功耗限制会重新调校；改变核心锁频会选用
（或调校一次）该锁频自己的 CMFD 配置（自 1.7.2 起）。MANUAL.md 第 13 节。

### Proxima（PROX），自 1.7.8 起

需要 NVIDIA 显卡（GTX 10 / RTX 20/30/40/50）。飞行表：币种 `PROX`（如果列表中没有，请添加为自定义币种），钱包 = 你的 PROX 地址（InnovLab：
你的 holder id，64 位十六进制），Pool *Configure in miner*，Miner *Custom* `freeforgem`（同一安装链接）。
**Hash algorithm：保留 `----`**：HiveOS 列表中没有 `proxima`。模板 `%WAL%.%WORKER_NAME%`（单独挖矿：`%WORKER_NAME%`）。

| | proximamine | InnovLab | 单独挖矿 |
|---|---|---|---|
| Pool URL | `stratum+ssl://pool.proximamine.com:3334` | `innov+tls://eu3.innovlab.cc:19701` | `http://NODE_IP:PORT` |
| Pass | 你的 proximamine secret | `x` | 节点的 `mine.seeker.token` |

所有字段的截图：`docs/img/hiveos-prox-*.png`（也在 MANUAL.md 第 21 节中）。
Extra config：`--auto-uv --lock-cclock 1500`（RTX 3070、RTX 3070 Laptop、RTX 3080 Ti；所有卡一个值，或按 GPU 用逗号分隔的列表）。
核心锁频决定速度，auto-uv 偏移决定功耗；推荐锁频（找到或固定的偏移）：RTX 3070 1500（+120，2.66 GH/s，87 W），RTX 3070 Laptop 1500（+270，
2.23 GH/s，53 W），RTX 3080 Ti 1500（+210，4.40 GH/s，147 W；追求速度用 1800 +270，5.28 GH/s，201 W），RTX 4070 Ti **2450**（+270，5.47 GH/s，
142 W）。CMP 50HX 未测（CMP 不能锁显存）。默认把显存锁在最低频率（`--no-mem-lock` 不改动显存）。网页界面中的 HiveOS 超频由 `nvidia-oc` 在每次
矿工启动和超频更改时应用，并会重置锁频和偏移：自 1.7.8 起矿工会重新设置它们（日志：`... was reset by an OC apply ... set again`，每块 GPU
每小时最多 4 次）。面板显示 GH/s 和 GH/W。矿池、选项和开发者抽水（1%）：MANUAL.md 第 21 节。

### NEB（Neural Engine Byte），自 1.8.0 起

飞行表：币种 `NEB`（如果列表中没有，请添加为自定义币种），钱包 `neb1...`，Pool *Configure in miner*，Miner *Custom* `freeforgem`（同一安装链接）：

| | Suprnova | 你的网关（多台矿机共用一个 NEB 节点） |
|---|---|---|
| Hash algorithm | `nebhash` | `nebhash` |
| Wallet and worker template | `%WAL%.%WORKER_NAME%` | `%WAL%.%WORKER_NAME%`（区块奖励归节点的 `--miner-lock`） |
| Pool URL | `stratum+tcp://neb.suprnova.cc:7804`（备用：`stratum+tcp://neb.suprnova.cc:7804,stratum+tcp://neb.suprnova.cc:7805`） | `neb://GATE_IP:3442` |
| Pass | `x` | 网关令牌 |
| Extra config arguments | 例如 `--auto-uv --lock-cclock 1800 --table-mclock 810` | 同左 |

- URL 本身选择 NEB；在 Extra config 中用 `--algo neb` 可为其他 NEB 矿池强制指定。
- 超频配置可以保留 Pearl 的显存锁频（810 MHz）：NEB 挖矿时显存以最高频率运行，矿工停止时恢复配置中的锁频。不要为 NEB 降低显存。
  `--table-mclock 810` 只在 NEB 不需要高速显存的地方降低它（RTX 3070：-10 W，MA/W +8-10 %）；`--tbig auto`（默认）在划算的地方使用更多显存。
  需要完整显存频率的模型会保持全频，这是正确的（在 810 MHz 下它会损失大部分算力）。自 1.8.1 起这种模型不再触发 `NO TABLE` 警报；
  该锁生效期间矿工屏幕中 `/810` 显示为青色。
- 引擎 nebgpu 0.8.4（自 1.8.1 起；Turing 上更快），每块 GPU 一个引擎进程：一块卡上的 CUDA 故障只重启这块卡（HiveOS 中每块 GPU 多显示一个 `freeforgem` 进程）。
- `--power-limit N`（`--pl N`）可选，默认从不开启：矿工用核心锁频和偏移来限制功耗。`--reboot-on-lost-gpu 10`（需手动开启）在某块 GPU 丢失
  （从总线掉线）10 分钟后重启矿机。
- HiveOS 显示 khs（MA/s x 1000）、15 分钟速率、版本旁的 MA/W、`LOCK NOT HELD`、`NO TABLE`、`GPU LOST`、`REBOOT NEEDED` 以及 NEB 显存警告。
- 用 `ffgn-hive` 挖过 NEB 的矿机保留其调好的偏移（首次启动时导入 `/hive-config/ffgn/uv.json`）。
- 整台矿机挖 NEB = 本飞行表；部分卡挖 NEB、其他卡挖别的币：见下文 *不同显卡挖不同的币*。详见 MANUAL.md 第 24 节。

### Ting / Daler（DLR），自 1.8.0 起

Ting 矿工 FFGT 自 1.8.0 起成为 FreeForGem 的一部分（相同的内核，相同的数字）。NVIDIA GTX 10 及更新（CUDA），AMD（OpenCL）。
飞行表：币种 `DLR`（添加为自定义币种），钱包 = 你的 Ting 账户（64 位十六进制，**不带** `&net=...`；Ting ID 应用或 `ting wallet account`），
Pool *Configure in miner*，Miner *Custom* `freeforgem`（同一安装链接）。
**Hash algorithm：保留 `----`**（HiveOS 列表中没有 `ting-sha256`）。模板 `%WAL%.%WORKER_NAME%`。

| | crypto-connection.net | 你的 ffgt-gate | 在本矿机的节点上单独挖矿 |
|---|---|---|---|
| Pool URL | `crypto-connection.net:4073` | `ting://GATE_IP:PORT` | `node://127.0.0.1:18440` |
| Pass | `x` | 矿机在网关上的密码 | -（Extra config `--node-dir DIR`） |

Extra config：`--lock-cclock 1800 --auto-uv`（RTX 3070：1500 = GH/W 最佳，1800 = 更高 GH/s）。默认把显存锁在最低频率（SHA-256 不使用显存：
功耗约 −10 %；`--no-mem-lock` 不改动显存；HiveOS 超频配置中的显存锁频会保留；CMP 50HX：驱动不支持锁显存）。分组搜索（默认）在 RTX 30 上单独
挖矿约 +40 %，矿池上约 +25 %（`--no-group` 关闭）。面板显示 GH/s 和 GH/W，algo 为 `ting`。每块 GPU 在自己的引擎进程中挖矿：一块卡上的 CUDA
故障只会在几秒内重启这块卡（HiveOS 中每块 GPU 多显示一个 `freeforgem` 进程；`--single-process` = 旧模式）。开发者抽水 1 %（每 100 分钟 60 秒）
在 crypto-connection.net 上。选项：`freeforgem --algo ting --help`。详见 MANUAL.md 第 26 节。

### 不同显卡挖不同的币（`--split`），自 1.8.0 起

整台矿机一个飞行表：**主币种**像平常一样在飞行表中设置（它的 Pool URL、Wallet 和 Pass），**另一个币种**写在 *Extra config arguments* 中，
数值带上该币种名称的标签（`--pool pearl=...`、`--wallet pearl=...`）。带标签的值会替换飞行表中该币种组的无标签值，因此飞行表的矿池和钱包
仍归主币种。

**示例：GPU0 和 GPU1 在 Suprnova 挖 NEB，GPU2 和 GPU3 在 Kryptex 挖 Pearl**（把占位钱包换成你自己的）：

1. **钱包。** 一个 NEB 钱包（`neb1...`）。PRL 地址写在 Extra config 中，因此不需要在 HiveOS 中添加钱包。
2. **飞行表：** 币种 `NEB`，NEB 钱包，Pool *Configure in miner*，Miner *Custom* → *Setup Miner Config*：

   | 字段 | 值 |
   |---|---|
   | Miner name | `freeforgem` |
   | Installation URL | 上面的链接（1.8.1） |
   | Hash algorithm | `nebhash` |
   | Wallet and worker template | `%WAL%.%WORKER_NAME%` |
   | Pool URL | `stratum+tcp://neb.suprnova.cc:7804` |
   | Pass | `x` |
   | Extra config arguments | `--split neb:0,1 pearl:2,3 --pool pearl=prl-eu.kryptex.network:7048 --wallet pearl=prl1你的PRL地址 --lock-cclock neb=1800 --lock-cclock pearl=1410 --table-mclock neb=810 --auto-uv` |

3. **超频：** 超频配置可以把所有卡的显存锁在 810：NEB 卡在挖矿时以全频显存运行（退出时恢复 810），Pearl 卡保持 810。核心锁频来自
   `--lock-cclock neb=...` / `pearl=...`。
4. 把飞行表**应用**到矿机。

Extra config 的规则：

- **分组：** `COIN:GPUS`，GPU 编号与面板一致（`neb:0,1 pearl:2,3`，范围 `neb:0-3`，可以有一个组取 `rest`：`neb:0,1 pearl:rest`）。
  空格、`,` 和 `;` 都是等价的分隔符，不需要引号。一块卡只挖一种币。
- **无标签的选项发给所有组**（`--auto-uv`、模板中的矿工名）；**带标签的**只发给该币种的组（`--lock-cclock pearl=1410`）。只有部分币种才有的
  选项（NEB 的 `--table-mclock`、`--pl`、`--tbig`；Ting 的 `--no-group`；……）会自动只发给这些币种的组；矿工日志中每组有一行 `split:`，
  说明该组没有收到哪些选项。
- **无标签的按卡列表针对整台矿机**，按组切分：`--lock-cclock 1800,1800,1410,1410` 给 NEB 1800,1800，给 Pearl 1410,1410。
- **HiveOS 按空格拆分 Extra config**，因此 `--split-args` 只接受一个单词：为某组的每个标志分别写 `--split-args neb=--auto-uv`，带值的选项用
  标签（`--table-mclock neb=810`，而不是 `--split-args neb="--table-mclock 810"`）。
- 飞行表的 Pool URL、Wallet 和 Pass（以及单独挖矿节点）由矿工的 HiveOS 脚本标记为飞行表的币种，因此只会到达该币种的组：网关密码绝不会
  发往另一种币的矿池。
- 给值加标签（`--table-mclock neb=810`）可以表达得更明确，或给两个组不同的值。

面板显示：

- 显卡最多的那个组的 **algo 和总算力**（显卡数量相同的组中，取 `--split` 中的第一个）；
- **每块卡**各自的速率；币种单位不同时，每块卡以其原始的每秒速率给出（NEB 为尝试次数/秒，Pearl 为哈希/秒），因此不要跨币种比较这些数字；
- 所有组合计的**接受 / 拒绝**份额；
- **版本列**列出每个组及其效率：`1.8.1 split: neb 1.40 MA/W + pearl 0.61 TH/W`，以及所属组的标记，如 `LOCK NOT HELD` 或 `GPU LOST`。

每个组作为自己的矿工进程运行，有自己的矿池会话、开发者抽水、频率、显存策略、调校器和重启。结束的组 10 秒后自行重启（10 分钟内结束 3 次后：
暂停 10 分钟），其他组继续挖矿。`miner stop` 停止所有组，每组恢复自己的频率。在矿工日志（`/var/log/miner/freeforgem/freeforgem.log`）中，
组的每一行以 `[neb]` / `[pearl]` 开头（矿工屏幕中为浅蓝色），监督进程自己的行以 `split:` 开头。已在一台 4 卡 HiveOS 矿机上测试
（2 NEB + 2 Pearl）。详见 MANUAL.md 第 27 节。

### 任意长度的按卡列表，自 1.8.0 起

每个按卡选项（`--lock-cclock`、`--lock-mclock`、`--moffset`、`--uv-offset`、`--uv-max`、`--uv-min`、`--temp-limit`、`--temp-stop`，NEB 的
`--table-mclock` 和 `--power-limit`）都接受任意长度的列表，就像 HiveOS 的超频列表：较短的列表把最后一个值给其余 GPU，超出最后一块 GPU 的值
被忽略，`-` / 空字段 / 在 0 无效处写 `0` = 不改动该 GPU。因此一个飞行表适用于显卡数量不同的矿机：在 8 卡矿机上 `--table-mclock 810,810,-,-`
= GPU0 和 GPU1 设为 810，其余六块不动（1.8.0 之前："4 values for 8 device(s)"，且无法启动）。MANUAL.md 第 28 节。

### 矿工屏幕：显卡表格，自 1.8.1 起

打开 Hive Shell（或 SSH）并输入 `miner`：矿工显示显卡表格，而不是滚动的日志：每块 GPU 的算力、温度、风扇 %、功耗、功耗限制、每瓦效率、频率和
锁频、V/F 偏移和份额，矿机合计；顶部是矿池和矿工名、`net`（区块、任务、ping、过期份额）、`mode`（算法、auto-uv、锁频）、你的选项（`args`，
机密信息被遮蔽），以及最新的重要事件（`!`，固定的 5 行区域）和实时警报（`!!`）。OFFSET 单元格：auto-uv 调校时为橙色 `► +300 ↑ 3/9`，失败的
步骤及其回退为橙色 `✗ -2` / `× roll-back`，调好后为绿色 `✓ -9.6%`，auto-uv 关闭时为黄色 `-`；`OUT 14m` = 显卡因反复故障被排除，会自行回来。
橙色表示调校在正常工作，红色才需要你动手。每一列、标记和颜色的含义：MANUAL.md 第 31 节。

- `miner log`、网页 “Miner log” 和 h-stats.sh 像以前一样读取日志：每一行都在，没有变化。
- h-run.sh 把矿工窗口设为 UTF-8，并关闭该窗口的 screen 日志，因此 `/run/hive/miner.N` 得到的是日志行，而不是重绘的屏幕。GNU screen 下只有
  16 种颜色：橙色看起来是暗黄色（网页 Hive Shell 中为棕橙色），✗ ✓ 显示为 × √。
- *Extra config arguments* 中的选项：`--ui lines` 旧的滚动日志；边框显示异常（奇怪字母代替线条）时用 `--ui ascii`；`--no-color` 不着色；
  在 shell 中运行 `--ui-demo split` 用虚构矿机显示表格。
- BAD 统计 GPU **在挖矿时**被 CPU 检查拦下的错误结果（从未发送；不是坏份额：被矿池拒绝的是 REJ）。auto-uv 某一步中的错误结果是保护机制在工作：
  OFFSET 单元格在该步期间显示它们（`+390 ✗ 15 wrong`），BAD 不计入。

**窗口保持打开（自 1.8.1 起）。** 出错退出后，h-run.sh 输出 `freeforgem exited (code N), restarting in 5 s`（屏幕、日志、`miner log`），
并在同一窗口中重新启动矿工；这种启动会跳过一次性参数（`--uv-reset`、`--uv-reset-all`、`--uv-unpause`、`--uv-parallel-reset`、
`--auto-lock-reset`）。`miner stop` / `miner restart`（包括 HiveOS 看门狗发出的）像以前一样结束循环。频繁退出后，
窗口会等待 10 分钟再启动；`miner restart` 立即启动。窗口只有在 `miner stop` / `miner restart` 时才关闭
（"[screen is terminating]"）。

### 故障和丢失的 GPU，自 1.8.0 起

- 崩溃后 auto-uv 从崩溃偏移**减 2 步**处继续（所有币种）。
- GPU 故障后，显卡先被置于**安全状态**（其中包括安全偏移），然后其引擎才重新启动。
- **FAULTY：** 10 分钟内在安全偏移上故障 3 次（NEB、Ting）：该卡暂停 10 分钟，然后获得新的引擎。它没有丢失，不需要重启：请检查它的频率、供电和
  转接卡（riser）。
- **DEAD / LOST：** 只有无法再打开的显卡、Xid 79（从总线掉线）或 NVML 再也看不到的显卡：它在面板上计为 0（绝不使用其最后的速率，因此 HiveOS
  看门狗看到的是真实总数），版本列显示 `GPU LOST` / `REBOOT NEEDED`。手动开启的 `--reboot-on-lost-gpu 10`（NEB）在这种情况持续 10 分钟后重启
  矿机（矿工运行的前 10 分钟内不会，且每小时最多一次）。
- **过期份额：** 矿池回答 "job not found" 的任务会立即丢弃；当矿池的回答迟到数秒时，日志显示 `WARNING: the pool answers late`（矿机网络丢包：
  请检查其网卡、网线和交换机）。

MANUAL.md 第 29 节。
