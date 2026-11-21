// FFGT (FreeForGem Ting) — see LICENSE
// tinggpu: Ting / Daler (DLR) proof-of-work search library, C ABI.  Closed-source kernel, loaded at run time by the ffgt host
// (host/src/ting/gpu.rs).  Two builds share this API: libtinggpu.so / tinggpu.dll (CUDA, NVIDIA sm_61+) and
// libtinggpu_cl.so / tinggpu_cl.dll (OpenCL, AMD RDNA and others).  Device numbers: CUDA ordinals (the host sets
// CUDA_DEVICE_ORDER=PCI_BUS_ID) or, for OpenCL, the GPU devices of every platform in platform order.
//
// PoW: BlockId = SHA-256("Ting/" || u16le(5) || "block" || header[294]); valid when BlockId, read big-endian, <= target.
// The 16-byte nonce is little-endian at header offset 278 (message byte 290): the first 4 SHA-256 blocks are fixed (midstate),
// the nonce, padding and length sit in the 5th.  A search fixes the nonce bytes 0..9 and 14..15 (`base`) and enumerates the
// 32-bit value g in bytes 10..13, big-endian (= message word 11 of the last block), so rounds 0..11 are computed once per call.
#pragma once
#include <stdint.h>
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
#define TG_API_VERSION 2
#define TG_HEADER_LEN 294
#define TG_NONCE_OFFSET 278
typedef struct tg_job_s {
    uint32_t midstate[8]; // SHA-256 state after message bytes 0..255
    uint32_t tail[16];    // the last block as big-endian words, nonce bytes zero, padding and length included
    uint32_t target[8];   // big-endian words
} tg_job_t;               // 128 bytes
int tg_api_version(void);
const char *tg_backend(void);              // "cuda" or "opencl"
const char *tg_last_error(void);
int tg_job_size(void);                     // sizeof(tg_job_t) = 128 (ABI check)
int tg_driver_version(void);               // CUDA driver version, or 0 for OpenCL; -1 without a driver
int tg_device_count(void);                 // -1 on error (tg_last_error)
int tg_device_name(int dev, char *out, int len);
int tg_device_pci_bus_id(int dev, char *out, int len); // "0000:01:00.0"
int tg_compute_capability(int dev);        // CUDA: 10*major+minor; OpenCL: 0
int tg_sm_count(int dev);                  // multiprocessors / compute units
// CPU: the job of a 294-byte header (its nonce bytes are ignored) and a 32-byte big-endian target.  0 = ok.
int tg_job_build(tg_job_t *job, const uint8_t *header294, const uint8_t *target32);
// CPU: BlockId of the header with `nonce16` (little-endian) in place, full SHA-256 of the tagged message (reference).
void tg_cpu_hash(const uint8_t *header294, const uint8_t *nonce16, uint8_t out32[32]);
// The nonce a search tries for g: `base16` with bytes 10..13 = g big-endian.
void tg_nonce(const uint8_t *base16, uint32_t g, uint8_t out16[16]);
// Scans g in [g_start, g_start + count) (count >= 1, g_start + count <= 2^32).  Candidates: first BlockId word <= target[0].
// Writes up to `max` candidate g values (any order) to `out`; returns the number found (may exceed max), or < 0 on an error.
// The host re-checks every candidate in full (tg_cpu_hash / its own SHA-256).
int tg_search(int dev, const tg_job_t *job, const uint8_t *base16, uint32_t g_start, uint32_t count, uint32_t *out, uint32_t max);
// Hash rate (hashes/s) of `secs` of searching on a target nothing meets; < 0 on an error.
double tg_bench(int dev, double secs);
// API 2: the same search with g in message word `word` of the last block: 11 = nonce bytes 10..13 (kernel t1, as tg_search),
// 10 = nonce bytes 6..9 (kernel t1p: a pool's extranonce1 in bytes 12..15 stays fixed), 9 = bytes 2..5.
int tg_search_w(int dev, const tg_job_t *job, const uint8_t *base16, int word, uint32_t g_start, uint32_t count, uint32_t *out, uint32_t max);
void tg_nonce_w(const uint8_t *base16, int word, uint32_t g, uint8_t out16[16]);
const char *tg_kernel_name(int dev);       // short id of the kernel variant in use on dev
#ifdef __cplusplus
}
#endif
