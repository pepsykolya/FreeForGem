# FreeForGemPear

GPU miner for **Pearl (PRL)** — pearlhash proof-of-useful-work — with a **1% developer fee**.

Our own CUDA kernel for NVIDIA Ampere: the A′ matrix is generated on-chip, the int8 tensor-core
GEMM (`mma.sync m16n8k32`) runs in a persistent kernel, and the jackpot fold and the BLAKE3
hash are fused into it. The main loop is hand-tuned at SASS level. The host builds and checks
every share proof with the official `zk-pow` verifier before it is submitted.

## Performance

RTX 3070 locked at 1560 MHz core / 810 MHz memory, LuckyPool, 5-minute run:

| | Hashrate | Power |
|---|---|---|
| FreeForGemPear 1.0.0 | 65.6 TH/s | ~105 W |

Kernel bench: 66.6 TMAC/s, 90.7% of the card's int8 tensor peak at that clock.

## Supported hardware

* **Tuned for:** RTX 30xx (Ampere, sm_86) — 3060 / 3060 Ti / 3070 / 3070 Ti / 3080 / 3090.
* **Should run untuned:** RTX 40xx (sm_89 runs sm_86 code), not tested yet.
* **Not supported:** older GPUs, A100 (sm_80), AMD.
* **System:** Linux x86_64, NVIDIA driver 525 or newer. The release binary is built on
  Ubuntu 20.04 (glibc 2.31), so it runs on Ubuntu 20.04+ and HiveOS.

## Usage

```
tar xzf freeforgempear-1.0.0-linux-x86_64.tar.gz
cd freeforgempear-1.0.0
./freeforgempear --self-test                      # optional: checks the GPU kernel against the CPU reference
./freeforgempear --pool pearl-eu2.luckypool.io:3360 --wallet YOUR_PRL_ADDRESS --worker rig1
```

| Option | Meaning |
|---|---|
| `--pool HOST:PORT` | stratum pool (default `pearl-eu2.luckypool.io:3360`) |
| `--wallet ADDRESS` | your PRL address (required) |
| `--worker NAME` | worker name (default `worker1`) |
| `--devices 0,1,...` | GPUs to use (default: all visible; honours `CUDA_VISIBLE_DEVICES`) |
| `--minutes N` | stop after N minutes (default: run until Ctrl-C / SIGTERM) |
| `--self-test` | verify the kernel and exit |

The miner reconnects to the pool on its own. Every 10 s it logs the hashrate per GPU and in total,
plus the submitted, accepted and rejected shares.

**Tuning.** For best efficiency on a 3070, lock the core clock (for example
`nvidia-smi -lgc 1560,1560`) and lower the memory clock (`nvidia-smi -lmc 810,810`). The kernel
works from L2 cache and barely uses VRAM bandwidth.

## Developer fee

1% of the mining time: one 60-second slot in every 100 minutes, at a random moment of the cycle,
on **the same pool you configured**, under a separate login (worker `ffgp`). The log shows
`devfee: mining 60s for FreeForGemPear dev (1%)` and `devfee: back to user`. If the fee login
can't be served, the slot is skipped; your mining is never blocked.

The fee is a condition of the license: removing, reducing or redirecting it is not permitted
(see [LICENSE](LICENSE), clause 2).

## Building from source

Requirements: CUDA toolkit 12.x, g++, Rust (stable, edition 2024), git.

```
./build.sh          # -> dist/freeforgempear-<version>-linux-x86_64.tar.gz
```

Portable build matching the release binaries (needs Docker):

```
docker build -f Dockerfile.build -t ffgp-build .
docker run --rm -v "$PWD":/src ffgp-build
```

`gpu/g13_patched_cubin.h` holds the SASS-tuned kernel. It is generated from `kernels/` by
`tools/sass/build_patched.sh`, which needs a [CuAssembler](https://github.com/cloudcores/CuAssembler)
checkout (MIT) and changes only instruction control codes. If the patched kernel can't be loaded,
or `PEARL_NO_PATCH=1` is set, the library falls back to the plain compiled kernel and says so in
the log.

## License

FreeForGemPear License 1.0 — free to use, study, modify and redistribute, **as long as the
developer fee stays intact**. See [LICENSE](LICENSE). Third-party components are listed in
[NOTICE](NOTICE).
