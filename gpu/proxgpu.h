// FreeForGem — see LICENSE
// proxgpu: Proxima (PROX, txid-blake2b: BLAKE2b-256(message || nonce_be8)) GPU search library, C ABI.  Closed-source kernel
// (ffgprox kernel A, embedded cubins sm_61..sm_120 + PTX), loaded at run time by the freeforgem host (host/src/prox/gpu.rs).
// Separate from libpearlgpu / libnoidgpu.  Device numbers are CUDA ordinals as the process sees them (the host sets
// CUDA_DEVICE_ORDER=PCI_BUS_ID, so they match nvidia-smi).  Thread-safe per device (one call at a time per device).
#pragma once
#include <stdint.h>
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
#define PX_API_VERSION 1
#define PROX_MAX_MSG 4096
// Job template (376 bytes, 8-aligned), built once per job on the CPU by prox_job_build (midstate of every block before the tail).
typedef struct prox_job_s {
    uint64_t midstate[8];
    uint64_t tail[32];
    uint32_t tail_blocks;
    uint32_t tail_len;
    uint64_t total_len;
    uint32_t nonce_offset;
    uint32_t k;             // required trailing zero bits of h[24..32] read big-endian
    uint8_t  beat[26];
    uint8_t  has_beat;
    uint8_t  _pad[5];
} prox_job_t;
int px_api_version(void);
const char *px_last_error(void);
int px_job_size(void);                      // sizeof(prox_job_t) = 376 (ABI check)
int px_driver_version(void);                // cudaDriverGetVersion, -1 without a driver
int px_device_count(void);                  // -1 on error (px_last_error)
int px_device_name(int dev, char *out, int len);
int px_device_pci_bus_id(int dev, char *out, int len);   // "0000:01:00.0"
int px_compute_capability(int dev);         // 10 * major + minor, -1 on error
int px_sm_count(int dev);
int prox_job_build(prox_job_t *job, const uint8_t *msg, size_t len, uint32_t k, const uint8_t *beat26);
// Scans the COUNTERS nonce_start .. nonce_start+count-1; a counter maps to a nonce by a job-specific permutation of the low
// 6 nonce bytes (bits 48..63 copied unchanged: the pool's extranonce1 stays), disjoint counter ranges = disjoint nonces.
// Returns the number of candidates (real nonces, >= min(k,32) zero bits; the host re-checks them) or < 0 on a CUDA error.
int prox_search(int dev, const prox_job_t *job, uint64_t nonce_start, uint64_t count, uint64_t *out_nonces, uint32_t max);
uint64_t prox_counter_to_nonce(const prox_job_t *job, uint64_t c);
void prox_cpu_hash(const uint8_t *msg, size_t len, uint64_t nonce, uint8_t out32[32]);
double prox_bench(int dev, double secs, uint32_t prefix_len, int verbose);
const char *prox_variant_name(const prox_job_t *job);
#ifdef __cplusplus
}
#endif
