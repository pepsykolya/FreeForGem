# Prebuilt GPU library

The FreeForGemPear GPU kernel is closed source. The miner host program (`host/`) is built from source and
links the prebuilt search library from this directory:

| File | Platform |
|---|---|
| `linux-x86_64/libpearlgpu.so` | Linux x86_64, glibc 2.31+ (built on Ubuntu 20.04) |
| `windows-x64/pearlgpu.dll`, `windows-x64/pearlgpu.lib` | Windows 10/11 x64 (static CUDA runtime and MSVC CRT; `.lib` is the import library) |

The library exports only the `pg_*` C API declared in `gpu/pearlgpu.h`. Needs an NVIDIA driver 525+ and an
sm_86+ GPU (tuned for Ampere). RTX 50xx (Blackwell, sm_120) runs a compiled kernel since 1.5.0 and needs driver 570+.
Turing (sm_75: RTX 20xx, GTX 16xx, CMP 30HX–50HX) runs its own SASS kernel since 1.5.3 (1.5.3-rc3; compiled fallback in rc1/rc2).
Ada (sm_89: RTX 40xx) runs a dedicated SASS kernel since 1.5.4 (256-row tiles: same hashrate, about 6% fewer watts per TH on an RTX 4070 Ti);
RTX 30xx (Ampere, sm_86) runs a revised SASS kernel since 1.5.4-rc2 (A′ built with ldmatrix.trans: about 2% fewer watts per TH on an RTX 3070,
−0.4% hashrate). Turing and RTX 50xx keep their kernels. `PEARL_KERNEL=gen13|t256|auto` forces the choice for A/B tests.
