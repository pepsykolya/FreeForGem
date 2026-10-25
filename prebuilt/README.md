# Prebuilt GPU library

The FreeForGem GPU kernel is closed source. The miner host program (`host/`) is built from source and
loads the prebuilt search library from this directory at run time (`--backend auto|cuda|amd`):

| File | Platform |
|---|---|
| `linux-x86_64/libpearlgpu.so` | Linux x86_64, glibc 2.31+ (built on Ubuntu 20.04) |
| `linux-x86_64/libnoidgpu.so`, `windows-x64/noidgpu.dll` | NOID (Parano1d) search library, since 1.7.0: NVIDIA sm_80+ (RTX 30/40/50, A100/H100), **driver R580+**; kernels for sm_80/86/89/90/120 embedded as cubins (ptxas 13.4, `clmad`); exports only `nd_*` (`gpu/noidgpu.h`); depends only on the NVIDIA driver (libcuda / nvcuda). Separate from the Pearl library, which is unchanged |
| `windows-x64/pearlgpu.dll`, `windows-x64/pearlgpu.lib` | Windows 10/11 x64 (static CUDA runtime and MSVC CRT; `.lib` is the import library) |
| `windows-x64/pearlgpu_amd.dll` | Windows 10/11 x64, AMD GPUs (OpenCL; RDNA2 and newer — tested on RX 6800 XT), since 1.6.0 |
| `windows-x64/amduv.exe` | Windows, AMD: clock/voltage tuner for `--auto-uv` (ADLX), since 1.6.0 |
| `windows-x64/cmfd/cmfd-v4-replay-ocl.exe`, `windows-x64/cmfd/cmfd-miner-ffgp.exe` | Windows, AMD: CMFD OpenCL worker and the CMFD pool client rebuilt with `cmfd/patches/` (see `cmfd/README.md`), since 1.6.0 |

The library exports only the `pg_*` C API declared in `gpu/pearlgpu.h`. Needs an NVIDIA driver 525+ and an
sm_86+ GPU (tuned for Ampere). RTX 50xx (Blackwell, sm_120) runs a compiled kernel since 1.5.0 and needs driver 570+.
Turing (sm_75: RTX 20xx, GTX 16xx, CMP 30HX–50HX) runs its own SASS kernel since 1.5.3 (1.5.3-rc3; compiled fallback in rc1/rc2).
Ada (sm_89: RTX 40xx) runs a dedicated SASS kernel since 1.5.4 (256-row tiles: same hashrate, about 6% fewer watts per TH on an RTX 4070 Ti);
RTX 30xx (Ampere, sm_86) runs a revised SASS kernel since 1.5.4-rc2 (A′ built with ldmatrix.trans: about 2% fewer watts per TH on an RTX 3070,
−0.4% hashrate). Turing and RTX 50xx keep their kernels. `PEARL_KERNEL=gen13|t256|auto` forces the choice for A/B tests.

The AMD library (`pearlgpu_amd.dll`, closed source like the CUDA one) exports the same `pg_*` API plus `pg_amd_power_w`; it
uses discrete AMD GPUs only (integrated Radeon graphics are skipped) and needs the AMD Adrenalin driver (OpenCL). No Linux AMD
build yet.
