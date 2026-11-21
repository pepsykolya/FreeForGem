# FreeForGem on HiveOS

[English](#english) · [Русский](#русский)

## English

FreeForGem runs on HiveOS as a **Custom miner**: no SSH and no manual installation.

**Moving from FreeForGemPear:** since 1.7.0 the multi-coin miner is **FreeForGem** (custom miner `freeforgem`). HiveOS treats it
as a different miner: **create a new flight sheet** with Miner name `freeforgem` and the URL below (the old `freeforgempear`
flight sheet keeps running the old miner). Saved `--auto-uv` results are taken over automatically on the first start (from
`/hive-config/freeforgempear/uv.json`). FreeForGemPear itself continues as the Pearl miner.

**Installation URL (1.8.0):**

```
https://github.com/pepsykolya/FreeForGem/releases/download/v1.8.0/freeforgem-1.8.0_hiveos.tar.gz
```

HiveOS caches the downloaded archive by its file name: every version has a new file name, so always paste the link of the new release.

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

The kernel works from L2 cache and needs almost no VRAM bandwidth:

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
- **The first tuning takes about 15 minutes per GPU** (10-25 depending on the number of steps; RTX 3070 since 1.7.2: about 11 min for an 8-GPU rig, all GPUs at once). Since 1.7.2 the rig mines meanwhile:
  only the GPU being tuned pauses. The result is kept in `/hive-config/freeforgem/uv.json` (survives miner updates and reboots), **one
  profile per core clock lock** (since 1.7.2): switching the lock in the OC profile reuses that lock's verified result, a new lock is
  tuned once; it is tuned again when the driver or the kernel build changes. A GPU that crashes while mining is named in the log and
  stepped back by the miner itself (`gpu-faults.json`; Linux / HiveOS: it restarts itself in the same process, at most 12 times an
  hour), see [MANUAL.md](MANUAL.md#17-profiles-per-clock-lock-mining-during-tuning-crashes-while-mining-new-in-172).
- Set your Overclock tab (core lock `1560`, memory, power limit) first. The tuner starts from the offset of the OC profile
  (RTX 3070: from a default by the core lock, see below; a start that fails steps down 30 MHz); an offset another miner left on
  the card is replaced by the OC profile value at start; it never touches locked clocks, power limit or memory. When HiveOS re-applies its OC while mining (at miner
  start or after an OC change it writes the *Core Clock* value, **0 when the field is empty**), auto-uv puts the verified offset
  back after a quick check (since 1.7.1; before, the undervolt was off until the next restart). A different offset set by hand
  makes it stand down on that GPU.
- `--lock-cclock` / `--lock-mclock` in *Extra config*: on exit the miner puts back the lock of your HiveOS OC profile (since
  1.7.1; before, it removed the lock and the rig ran unlocked).
- It touches only the GPUs the miner mines on (matched by PCI bus id, shown in the log). Every change is logged as
  `auto-uv: GPU0 (bus 0d, RTX 3070) offset +150 -> +195, 116.6 W -> 106.6 W, saved`; `grep auto-uv` in the miner log shows all of it.
- Stopping the miner restores your offset. If the miner was killed, the next start restores it first.
- On an RTX 3070 the tuning starts near the edge, by the core lock (Pearl +210 at 1805, +270 at 1410; NOID eff +105 at 1400, speed +240 at 1500 .. +210 at 1750; CMFD +150; NOID climbs in 30 MHz steps), and
  stops at the voltage floor; `--uv-min N` overrides it. The miner screen shows the progress per GPU and colours (`--no-color` to turn off).
- **Reset:** add `--uv-reset` for one start: it forgets the profile of the current lock only (`--uv-reset-all`: every profile;
  or delete `/hive-config/freeforgem/uv.json`). On exit the offset of the OC profile (*Core Clock*) is put back.
  Other options (`--uv-offset N`, `--uv-max N`, `--uv-reval-hours H`, `--auto-uv-dry-run`) and a manual-tuning
  procedure are in [MANUAL.md](MANUAL.md#12-undervolt---auto-uv-optional-new-in-140). Undervolting is at your own risk, like any overclocking.
- **Mixed rig:** `--auto-uv` is enough — the limits are chosen per card (since 1.6.2): RTX 20xx / GTX 16xx up to +225,
  CMP 30HX/40HX up to +150 (CMP 50HX: +330), all others up to +495; `grep range` in the miner log shows each GPU's range. For manual
  control give one value per GPU in nvidia-smi order (GPU0, GPU1, ... as in the miner log; or in `--devices` order), no spaces:
  `--auto-uv --uv-max 225,225,495,495,495,150` (`-` = that GPU's default). Since 1.8.0 a list may be shorter than the number of
  GPUs: the last value goes to the remaining ones (see *Per-GPU lists* below).

### Automatic core lock (optional, `--auto-lock`, new in 1.7.8)

Add `--auto-lock` to *Extra config arguments* (Pearl, NOID, PROX; not CMFD). The miner then sweeps the core clock locks of every GPU
(default 1800 ... 1200 MHz, highest first, each at its own `--auto-uv` profile, 45 s + 120 s of real mining per lock, shares go to the pool),
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
  (the 0.7-0.8 GB job matrix is built only then; until 1.7.3 every job kept it and every proof copied it). A `--auto-uv` check
  process needs ~0.25 GB, and the checks share one ~1.6 GB proof buffer (RTX 3070 / CMP 50HX; they prove one at a time). The miner plans its checks with all of the free RAM but 0.5 GB (70 % of it on a low-RAM rig): it logs "checking N of M
  GPU(s) at a time", and when even one does not fit (a 4 GB rig) it runs no checks or tunings: a GPU with a verified profile of
  its clocks uses it, the others mine at your offset. A check process is the first one the Linux OOM killer takes; a check killed
  for lack of RAM is not counted as a failed offset. Without `--lock-cclock` / `--lock-mclock` the miner sets the OC profile's
  locks before mining (another program may have left the clocks unlocked), and the GPUs start one by one, 1.5 s apart.
- **HiveOS reboots the rig at a high load average, even with its watchdog off** (`/hive/bin/wd`, check_la: `sreboot fast` when the
  5-min load average reaches the rig's max_la, 28 on a 4C/8T Xeon rig). Weak CPUs (4 cores) with many GPUs are at risk while
  several GPUs tune at once. Since 1.7.2 the miner keeps its runnable threads low (no busy wait for the GPU, one CPU reference
  at a time at nice 10) and the tuning summary shows the load average, with a WARNING above 2 x the CPU threads.

### CMP cards: unlocked driver (optional, off by default, 1.7.7)

Under the stock NVIDIA driver a CMP card (CMP 50HX and the family) makes about a tenth of its speed. To let the miner install an
unlocked driver, add the word `cmp-unlock-driver` to *Extra config arguments* (the miner does not get it). Before mining it checks, installs the driver
only if the unlocked build is not in place yet, and **reboots the rig**; a rig that already has it, has no CMP card or runs without root is left
alone. **This replaces the NVIDIA kernel driver of the whole rig:** read MANUAL.md section 20 first. By hand: `sudo ./freeforgem --cmp-driver`
(`FFG_CMP_DRIVER_DRY_RUN=1` only reports what it would do).

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

### Developer fee

1%: 60 s in every 100 minutes, on the same pool you set. See [LICENSE](LICENSE).
Since 1.8.0 every coin's fee runs on a connection of its own: NEB on your pool list (solo / gate: the public NEB pools), Ting on
crypto-connection.net, PROX on proximamine, CMFD inside the engine (36 s per hour per GPU). With `--split` every group pays 1 % of
its own cards' time. A slot whose connection was not ready is tried again up to 3 times.

### NOID (Parano1d), since 1.7.0

Needs NVIDIA RTX 30/40/50 and **driver 580+** (`nvidia-driver-update 580`). Flight sheet: coin `NOID`, wallet `o1...`, Miner
*Custom* `freeforgem` (same Installation URL), Hash algorithm `parano1d`, template `%WAL%.%WORKER_NAME%`, Pool URL
`stratum+ssl://eu2.innovlab.cc:19601`, Extra config `--algo noid --lock-cclock 1300 --auto-uv` (eff mode, best MH/W on RTX 30;
`--noid-mode speed --lock-cclock 1500 --auto-uv` for ~31 % more MH/s at ~4 % lower MH/W; 8x RTX 3070 table: MANUAL.md, section 15). Without `--auto-uv` a positive HiveOS core offset is set to 0 for NOID
(a Pearl undervolt gives silently wrong NOID results) and put back when the miner stops. Results: MANUAL.md, sections 15 and 16.
At the start the innovlab vardiff begins with an easy share target and the pool's replies can lag several seconds; since
1.7.2 at most 12 shares wait for a reply per connection, the best ones go first and easier surplus ones are skipped
(stats line: `rej N (stale N, skipped N)`); a block share is always sent.

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
bit-for-bit on the real model; first start ~15–20 min, then applied at once; owner's offset back on `miner stop`). Tune with the
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
  put back when the miner stops. Do not lower the memory for NEB. `--table-mclock 810` lowers it only on the pure digest-table
  path (RTX 3070: -10 W, +8-10 % MA/W); `--tbig auto` (default) builds a partial L=3 table where it pays back.
- Engine nebgpu 0.8.1, one engine process per GPU: a CUDA fault on one card restarts only that card (HiveOS shows one more
  `freeforgem` process per GPU).
- `--power-limit N` (`--pl N`) is optional and never on by default: the miner limits the power with the core lock and the offset.
  `--reboot-on-lost-gpu 10` (opt-in) reboots the rig when a GPU has been lost (fallen off the bus) for 10 minutes.
- HiveOS shows khs (MA/s x 1000), the 5-minute rate, MA/W next to the version, `LOCK NOT HELD`, `NO TABLE`, `GPU LOST`,
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

Extra config: `--lock-cclock 1800 --auto-uv` (RTX 3070: 1500 = best GH/W, 1800 = more GH/s; Turing / CMP get a lower auto-uv ceiling
built in). The memory is locked at its lowest clock by default (SHA-256 does not use it: about -10 % power; `--no-mem-lock` leaves it; a
memory lock of the HiveOS OC profile is kept; CMP 50HX: the driver has no memory lock). Templates of one tip (solo, gate) or jobs of one
height (pool) are searched together (about +40 % solo, +25 % pool on RTX 30; `--no-group` turns it off). The dashboard shows GH/s and
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
   | Installation URL | the link above (1.8.0) |
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
- the **version column** lists every group with its efficiency: `1.8.0 split: neb 1.40 MA/W + pearl 0.61 TH/W`, plus tags such as
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

### Faults and lost GPUs, since 1.8.0

- After a crash auto-uv resumes at the crashed offset **minus 2 steps** (every coin).
- After a GPU fault the card first gets a **safe state** (the driver's reset, a probe, its clocks and a safe offset), and only
  then its engine starts again.
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

**Ссылка для установки (1.8.0):**

```
https://github.com/pepsykolya/FreeForGem/releases/download/v1.8.0/freeforgem-1.8.0_hiveos.tar.gz
```

HiveOS кэширует скачанный архив по имени файла: у каждой версии новое имя файла, поэтому всегда вставляйте ссылку нового релиза.

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

Ядро работает из L2-кэша, видеопамять ему почти не нужна:

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
- **Первый подбор занимает около 15 минут на карту** (10-25 в зависимости от числа шагов; RTX 3070 с 1.7.2: около 11 мин на риг из 8 карт, все карты сразу). С 1.7.2 риг при этом майнит: на паузе
  только подбираемая карта. Результат хранится в `/hive-config/freeforgem/uv.json` (переживает обновления майнера и перезагрузки),
  **отдельный профиль на каждый лок ядра** (с 1.7.2): смена лока в профиле разгона берёт проверенный результат этого лока, новый лок
  подбирается один раз; при смене драйвера или сборки ядра подбор повторяется. Карту, падающую в майнинге, майнер называет в логе и
  сам понижает (`gpu-faults.json`; в Linux / HiveOS перезапускает себя в том же процессе, не больше 12 раз в час), см.
  [MANUAL.md, раздел 17](MANUAL.md#17-профили-по-локу-частоты-майнинг-во-время-тюнинга-падения-в-майнинге-новое-в-172).
- Сначала задайте вкладку Overclock (фиксация ядра `1560`, память, лимит мощности). Подбор стартует с offset профиля разгона
  (RTX 3070 — со значения по локу ядра, см. ниже; старт, не прошедший проверку, понижается на 30 МГц); offset, оставленный на
  карте другим майнером, при старте заменяется значением профиля; зафиксированные частоты, лимит мощности и память он не трогает.
  Если HiveOS заново применит разгон во время майнинга (при старте майнера и после смены OC он записывает значение *Core Clock*,
  **0, если поле пустое**), auto-uv после быстрой проверки поставит проверенный offset обратно (с 1.7.1; раньше андервольт
  выключался до перезапуска). Другой offset, выставленный вручную, — и auto-uv отступит на этой карте.
- `--lock-cclock` / `--lock-mclock` в *Extra config*: при выходе майнер возвращает лок из профиля разгона HiveOS (с 1.7.1;
  раньше лок снимался, и риг работал без него).
- Затрагиваются только карты, на которых работает майнер (сопоставление по PCI-адресу, он виден в логе). Каждое изменение
  пишется в лог так: `auto-uv: GPU0 (bus 0d, RTX 3070) offset +150 -> +195, 116.6 W -> 106.6 W, saved`; `grep auto-uv` по логу майнера покажет всё.
- При остановке майнера ваш offset возвращается. Если майнер был убит, при следующем запуске он восстанавливается первым делом.
- На RTX 3070 тюнинг начинается рядом с краем, по локу ядра (Pearl +210 при 1805, +270 при 1410; NOID eff +105 при 1400, speed +240 при 1500 .. +210 при 1750; CMFD +150; NOID поднимается шагами по 30 МГц), и
  останавливается на пороге напряжения; `--uv-min N` задаёт старт. Экран майнера показывает прогресс по картам и цвета (`--no-color` — без цвета).
- **Сброс:** добавьте `--uv-reset` на один запуск: он забывает только профиль текущего лока (`--uv-reset-all` — все профили; или
  удалите `/hive-config/freeforgem/uv.json`). При выходе возвращается offset профиля разгона (*Core Clock*).
  Остальные параметры (`--uv-offset N`, `--uv-max N`, `--uv-reval-hours H`, `--auto-uv-dry-run`) и порядок ручной настройки —
  в [MANUAL.md](MANUAL.md#12-андервольт---auto-uv-необязательно-новое-в-140). Андервольт — на ваш риск, как и любой разгон.
- **Смешанный риг:** достаточно `--auto-uv` — пределы выбираются по каждой карте (с 1.6.2): RTX 20xx / GTX 16xx до +225,
  CMP 30HX/40HX до +150 (CMP 50HX: +330), остальные до +495; `grep range` в логе майнера показывает диапазон каждой карты. Для ручного
  управления — по значению на карту в порядке nvidia-smi (GPU0, GPU1, ... как в логе майнера; или в порядке `--devices`), без пробелов:
  `--auto-uv --uv-max 225,225,495,495,495,150` (`-` = значение по умолчанию для карты). С 1.8.0 список может быть короче числа
  карт: оставшиеся получают последнее значение (см. *Списки по картам* ниже).

### Автовыбор лока ядра (по желанию, `--auto-lock`, новое в 1.7.8)

Добавьте `--auto-lock` в *Extra config arguments* (Pearl, NOID, PROX; не CMFD). Майнер переберёт локи ядра каждой карты (по умолчанию
1800 ... 1200 МГц сверху вниз, каждый со своим профилем `--auto-uv`, 45 с + 120 с настоящего майнинга на лок, шары уходят в пул), оставит
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
  (матрица задания 0,7-0,8 ГБ строится только тогда; до 1.7.3 каждое задание её хранило, а каждое доказательство копировало).
  Процессу проверки `--auto-uv` нужно ~0,25 ГБ, а буфер доказательства ~1,6 ГБ у всех проверок один (RTX 3070 / CMP 50HX; доказывают по очереди). Майнер планирует проверки по всей свободной памяти, кроме 0,5 ГБ (на риге с малой памятью по 70 %): в
  логе "checking N of M GPU(s) at a time", а если не помещается даже одна (риг с 4 ГБ) — проверок и тюнинга нет: карта с
  проверенным профилем своих частот использует его, остальные майнят на вашем offset. Процесс проверки OOM killer Linux убивает
  первым; проверка, убитая из-за нехватки памяти, не считается провалом offset. Без `--lock-cclock` / `--lock-mclock` майнер
  ставит локи профиля OC до начала майнинга (другая программа могла оставить частоты без лока), а карты стартуют по очереди,
  через 1,5 с.
- **HiveOS перезагружает риг при высоком load average, даже с выключенным вотчдогом** (`/hive/bin/wd`, check_la: `sreboot fast`,
  когда load average за 5 мин доходит до max_la рига, 28 на риге с Xeon 4C/8T). Слабые CPU (4 ядра) с многими картами
  рискуют, пока тюнятся несколько карт сразу. С 1.7.2 майнер держит мало готовых к исполнению потоков (без активного ожидания
  GPU, одна CPU-сверка за раз с nice 10), а сводка тюнинга показывает load average с WARNING выше 2 x числа потоков CPU.

### NOID (Parano1d), с 1.7.0

Нужны NVIDIA RTX 30/40/50 и **драйвер 580+** (`nvidia-driver-update 580`). Полётный лист: монета `NOID`, кошелёк `o1...`, Miner
*Custom* `freeforgem` (та же ссылка), Hash algorithm `parano1d`, шаблон `%WAL%.%WORKER_NAME%`, Pool URL
`stratum+ssl://eu2.innovlab.cc:19601`, Extra config `--algo noid --lock-cclock 1300 --auto-uv` (режим eff, лучшие MH/Вт на RTX 30;
`--noid-mode speed --lock-cclock 1500 --auto-uv` — примерно на 31 % больше MH/s при MH/Вт примерно на 4 % ниже; таблица рига 8x RTX 3070: MANUAL.md, раздел 15). Без `--auto-uv` положительный offset ядра из OC HiveOS на время NOID
ставится в 0 (андервольт Pearl даёт тихо неверные результаты NOID) и возвращается при остановке. Подробности: MANUAL.md,
разделы 15 и 16.
На старте vardiff innovlab начинает с лёгкой цели шары, и ответы пула могут отставать на секунды; с 1.7.2 ответа ждут не
больше 12 шар на соединение, лучшие уходят первыми, лишние более лёгкие пропускаются (строка статистики: `rej N (stale N,
skipped N)`); шара, подходящая под цель блока, отправляется всегда.

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
проверяется побитно на реальной модели; первый запуск ~15–20 мин, дальше применяется сразу; ваше смещение возвращается при
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
  майнера лок профиля возвращается. Не опускайте память для NEB. `--table-mclock 810` опускает её только на чистом пути таблицы
  дайджестов (RTX 3070: −10 Вт, +8–10 % MA/W); `--tbig auto` (по умолчанию) строит частичную таблицу L=3 там, где она окупается.
- Движок nebgpu 0.8.1, свой процесс движка на каждую карту: сбой CUDA на одной карте перезапускает только её (в HiveOS на каждую
  карту виден ещё один процесс `freeforgem`).
- `--power-limit N` (`--pl N`) — только по желанию и по умолчанию никогда не включается: майнер ограничивает мощность локом ядра и
  offset. `--reboot-on-lost-gpu 10` (по желанию) перезагружает риг, если карта потеряна (отвалилась от шины) 10 минут.
- HiveOS показывает khs (MA/s x 1000), скорость за 5 минут, MA/W рядом с версией, `LOCK NOT HELD`, `NO TABLE`, `GPU LOST`,
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

Extra config: `--lock-cclock 1800 --auto-uv` (RTX 3070: 1500 — лучшие GH/W, 1800 — больше GH/s; для Turing / CMP потолок auto-uv ниже,
он встроен). Память по умолчанию лочится на самой низкой частоте (SHA-256 её не использует: около −10 % мощности; `--no-mem-lock`
оставляет её; лок памяти из OC-профиля HiveOS сохраняется; у CMP 50HX драйвер не даёт лочить память). Шаблоны одной вершины (соло, шлюз)
или задания одной высоты (пул) перебираются вместе (около +40 % в соло, +25 % на пуле на RTX 30; `--no-group` выключает). На панели —
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
   | Installation URL | ссылка выше (1.8.0) |
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
- **колонка версии** — каждая группа со своей эффективностью: `1.8.0 split: neb 1.40 MA/W + pearl 0.61 TH/W`, плюс метки вроде
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

### Сбои и потерянные карты, с 1.8.0

- После падения auto-uv продолжает с упавшего offset **минус 2 шага** (все монеты).
- После сбоя карта сначала получает **безопасное состояние** (сброс драйвера, проверка, её частоты и безопасный offset), и только
  потом её движок стартует снова.
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
(`FFG_CMP_DRIVER_DRY_RUN=1` только сообщает, что было бы сделано).

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

### Комиссия разработчика

1%: 60 секунд из каждых 100 минут, на том же пуле, что указан у вас. Подробнее — в [LICENSE](LICENSE).
С 1.8.0 комиссия каждой монеты идёт по своему соединению: NEB — на вашем списке пулов (соло / шлюз: публичные пулы NEB), Ting — на
crypto-connection.net, PROX — на proximamine, CMFD — внутри движка (36 с в час на карту). С `--split` каждая группа платит 1 % времени
своих карт. Если соединение к слоту не готово, слот повторяется до 3 раз.
