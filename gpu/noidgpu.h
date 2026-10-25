// FreeForGemPear — see LICENSE
// noidgpu: NOID (Parano1d, Poseidon2b over GF(2^128)) GPU search library, C ABI.  Closed-source kernels (embedded cubins,
// sm_80+), loaded at run time by the freeforgempear host (host/src/noid/gpu.rs).  Separate from libpearlgpu: the Pearl library is
// not touched.  Every function returns 0 / a non-negative count on success and -1 on error (nd_last_error() says why);
// the state (device, kernel, job, launch slots) is per calling thread: one thread per GPU.
#pragma once
#include <stdint.h>
#ifdef _WIN32
#define ND_API __declspec(dllexport)
#else
#define ND_API __attribute__((visibility("default")))
#endif
#ifdef __cplusplus
extern "C" {
#endif
#define ND_API_VERSION 1
#define ND_MODE_EFF 0    // --noid-mode eff: best MH/W
#define ND_MODE_SPEED 1  // --noid-mode speed: best MH/s
ND_API int nd_api_version(void);
ND_API const char *nd_last_error(void);
ND_API int nd_driver_version(void);  // cuDriverGetVersion (13000 = R580 / CUDA 13.0), -1 without a driver
ND_API int nd_device_count(void);
ND_API int nd_device_name(int device, char *out, int len);
ND_API int nd_device_pci_bus_id(int device, char *out, int len);
ND_API int nd_device_uuid(int device, char *out, int len);
ND_API int nd_sm_count(int device);
ND_API int nd_compute_capability(int device);  // 10 * major + minor
// Label of the kernel nd_init(device, mode) would load ("eff:n2gc/sm86"), or "" when the device is not supported (sm < 80).
ND_API const char *nd_kernel_label_for(int device, int mode);
// Binds the calling thread to `device` and loads the kernel of `mode` (NOID_KERNEL=<variant> in the environment overrides it).
ND_API int nd_init(int device, int mode);
ND_API const char *nd_kernel_label(void);
ND_API int nd_grid_nonces(void);  // nonces per iteration of one launch (blocks x threads)
ND_API int nd_max_hits(void);     // hits stored per launch at most
// Job: the 16 PoW fields (256 bytes, field 10 = nonce).  Nonce bytes 164..175 are fixed for the job, bytes 160..163 are the
// kernel's 32-bit index (the `base` of nd_launch plus the thread's offset).  `target` (32 bytes LE): the kernel reports every
// nonce whose digest bytes 24..31 (as u64 LE) are <= the target's bytes 24..31 (candidates: the full compare is the caller's).
// Stream-ordered with nd_launch: launches queued before keep the old job.
ND_API int nd_set_job(const uint8_t fields[256], const uint8_t target[32]);
// Queues one search over nonces base .. base + iters * nd_grid_nonces() - 1 (32-bit index, wraps) into slot 0 or 1.
ND_API int nd_launch(int slot, uint32_t base, int iters);
// Waits for the launch in `slot`; returns the number of candidates found (may exceed max_hits / nd_max_hits()),
// stores min(count, max_hits, nd_max_hits()) nonce indexes (unsorted) in `hits`.
ND_API int nd_wait(int slot, uint32_t *hits, int max_hits);
// GPU digests (32 bytes each) of nonce indexes start .. start + n - 1 of the current job (self-test; synchronous).
ND_API int nd_digest(uint32_t start, uint32_t n, uint8_t *out);
ND_API void nd_release(void);
#ifdef __cplusplus
}
#endif
