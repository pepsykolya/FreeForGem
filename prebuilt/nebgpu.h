/* nebgpu — FFGN GPU engine for nebhash v2 (NEB, Neural Engine Byte).  C API, version 1.
 *
 * Contract (frozen 2026-11-16, API 1; later changes only backward compatible, logged in FFGN-JOURNAL.md):
 *  - One context = one CUDA device. A context is used by ONE host thread at a time, except nebgpu_cancel()
 *    which may be called from any thread. Several contexts (one per device) may run in parallel threads.
 *  - All functions return 0 on success or a negative NEBGPU_E_* code; nebgpu_last_error(ctx) gives text.
 *  - Datasets are the NMD2 datasets of neb-datasets.bin, addressed by their index in the file (the first
 *    byte of the node's Model frame). Mainnet: index 0 = KEP1 (arch 1), index 1 = RET2 (arch 2).
 *  - A model is set per dataset index. set_model builds the per-sample table on the device (blocking,
 *    ~0.1-1 s); setting the same weights again is a no-op. Up to NEBGPU_MAX_DATASETS indices are kept.
 *  - scan() evaluates nonces nonce_start .. nonce_start+count-1 (mod 2^32) of an 80-byte header (nonce
 *    written little-endian into bytes 76..79; the caller's bytes 76..79 are ignored) and reports every nonce
 *    whose ticket <= share_target (256-bit little-endian compare), flagging those also <= target.
 *  - Hits are candidates: the host MUST re-check each one with the CPU reference (nebgpu_ref_ticket) before
 *    sending, as the reference miner does.
 *
 * Header-only types; link with nebgpu.dll / libnebgpu.so. */
#ifndef NEBGPU_H
#define NEBGPU_H

#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32)
#if defined(NEBGPU_BUILD)
#define NEBGPU_API __declspec(dllexport)
#else
#define NEBGPU_API __declspec(dllimport)
#endif
#else
#define NEBGPU_API __attribute__((visibility("default")))
#endif

#ifdef __cplusplus
extern "C" {
#endif

#define NEBGPU_API_VERSION 1

#define NEBGPU_ARCH_KEP1 1 /* 784 inputs, 26 000 weights, packed 13 000 bytes */
#define NEBGPU_ARCH_RET2 2 /* 4096 inputs, 22 280 weights, packed 11 140 bytes */
#define NEBGPU_MAX_DATASETS 8
#define NEBGPU_NB 16 /* samples per attempt */

/* error codes */
#define NEBGPU_OK 0
#define NEBGPU_E_ARG -1         /* bad argument */
#define NEBGPU_E_CUDA -2        /* CUDA runtime/driver error (text in last_error) */
#define NEBGPU_E_NOMEM -3       /* host or device memory */
#define NEBGPU_E_UNSUPPORTED -4 /* device compute capability < 6.1, or arch unknown */
#define NEBGPU_E_NODATA -5      /* dataset index not loaded */
#define NEBGPU_E_NOMODEL -6     /* no model set for the dataset index */
#define NEBGPU_E_CANCELLED -7   /* scan stopped by nebgpu_cancel (see *done) */
#define NEBGPU_E_SELFTEST -8    /* self-test mismatch against the CPU reference */
#define NEBGPU_E_FORMAT -9      /* malformed dataset file / packed weights */
#define NEBGPU_E_COMPUTE -10    /* GPU computed a wrong result (canary / table check vs CPU reference): card unstable
                                   (overclock/undervolt too aggressive, overheating) -> host should back off OC and
                                   call nebgpu_set_model* again (the table is rebuilt) */

typedef struct nebgpu_ctx nebgpu_ctx;

typedef struct {
  int index;            /* CUDA ordinal */
  char name[128];
  char pci_bus_id[32];  /* "0000:01:00.0" */
  int cc_major, cc_minor;
  int sm_count;
  int clock_mhz;        /* max SM clock */
  uint64_t mem_total, mem_free;
  int supported;        /* 1 if the engine runs on it (CC >= 6.1; tested: 8.6, 8.9) */
  int tensor_int8;      /* 1 if the int8 tensor-core path is used (CC >= 8.0) */
} nebgpu_device_info;

typedef struct {
  uint32_t nonce;
  uint32_t flags;       /* bit 0: ticket <= share_target (always set); bit 1: ticket <= target (block) */
  uint8_t ticket[32];   /* the ticket as computed on the GPU (BLAKE3 output byte order) */
} nebgpu_hit;

#define NEBGPU_HIT_SHARE 1u
#define NEBGPU_HIT_BLOCK 2u

/* ---- library / devices ---- */
NEBGPU_API int nebgpu_api_version(void);                 /* NEBGPU_API_VERSION of the binary */
NEBGPU_API const char *nebgpu_build_info(void);          /* "nebgpu <ver> (<git>) cuda <ver> sm_75..." */
NEBGPU_API int nebgpu_device_count(void);                /* >= 0, or negative error */
NEBGPU_API int nebgpu_device_info_get(int device, nebgpu_device_info *out);

/* ---- context ---- */
NEBGPU_API int nebgpu_open(int device, nebgpu_ctx **out);
NEBGPU_API void nebgpu_close(nebgpu_ctx *ctx);
/* Last error text of the context (or of the calling thread when ctx is NULL). Valid until the next call. */
NEBGPU_API const char *nebgpu_last_error(const nebgpu_ctx *ctx);

/* ---- datasets ---- */
/* Load the whole neb-datasets.bin image ("NMDS" || LE32(count) || datasets); every NMD2 dataset i is loaded at
 * index i (NMD1 datasets are skipped). The buffer can be freed after the call. */
NEBGPU_API int nebgpu_load_datasets_file(nebgpu_ctx *ctx, const uint8_t *file, size_t len);
/* Load one dataset at an index: arch, n samples, records = n * (nin + 1) bytes (nin values 0..7, then label 0..1). */
NEBGPU_API int nebgpu_load_dataset(nebgpu_ctx *ctx, int ds_index, uint32_t arch, uint32_t n, const uint8_t *records);

/* ---- model ---- */
/* Weights in model order (spec nebhash-v2 section 3), int8 in -8..7, count = 26000 (KEP1) or 22280 (RET2). */
NEBGPU_API int nebgpu_set_model(nebgpu_ctx *ctx, int ds_index, const int8_t *weights, uint32_t nweights);
/* Packed weights exactly as in the Model frame (nibbles, low nibble first; 13000 or 11140 bytes). */
NEBGPU_API int nebgpu_set_model_packed(nebgpu_ctx *ctx, int ds_index, const uint8_t *packed, size_t len);

/* ---- mining ---- */
/* Scan nonces. hits[0..min(*nhits,max_hits)) are filled; *nhits = number found (may exceed max_hits: then
 * rescan a smaller range). *done (may be NULL) = nonces actually evaluated (== count unless cancelled).
 * target or share_target may be NULL (= never matches). Typical count: 2^18 .. 2^22 per call (~0.2-1 s). */
NEBGPU_API int nebgpu_scan(nebgpu_ctx *ctx, int ds_index, const uint8_t header[80], uint32_t nonce_start,
                           uint32_t count, const uint8_t target[32], const uint8_t share_target[32],
                           nebgpu_hit *hits, uint32_t max_hits, uint32_t *nhits, uint32_t *done);
/* Make the scan RUNNING on this context return early with NEBGPU_E_CANCELLED (*done = nonces evaluated).
 * Any thread. A cancel issued while no scan runs has no effect on later scans (since 0.2.6; before it was kept
 * for the next scan). Safe to call concurrently with or after nebgpu_close of the same context: then it does
 * nothing (the library keeps a registry of open contexts; the pointer is never dereferenced once closed). */
NEBGPU_API void nebgpu_cancel(nebgpu_ctx *ctx);

/* Evaluate n explicit attempt keys on the GPU (tests / audits): digests, tickets (n*32 bytes each) and the
 * 16 sample indices (n*16 u32); any output pointer may be NULL. */
NEBGPU_API int nebgpu_eval_keys(nebgpu_ctx *ctx, int ds_index, const uint8_t *keys, uint32_t n, uint8_t *digests,
                                uint8_t *tickets, uint32_t *idx);

/* Self-test: synthetic datasets of both architectures, several models (initial, trained, extremes, random),
 * GPU vs the embedded CPU reference (digest, ticket, indices) plus a header scan check. Does not touch the
 * loaded datasets/models. report (may be NULL) gets a one-line summary. Returns 0 or NEBGPU_E_SELFTEST. */
NEBGPU_API int nebgpu_selftest(nebgpu_ctx *ctx, char *report, size_t report_len);

/* ---- reliability / tuning (for auto-uv, auto-lock, overclock fault detection) ----
 * Built-in error detection (on by default):
 *  - after every table build, NEBGPU_OPT_VERIFY_ROWS random table rows are recomputed by the CPU reference and
 *    compared; at set_model the CPU reference also computes the tickets of 2 canary keys of the model.
 *  - every NEBGPU_OPT_CANARY_EVERY scan calls (default 1 = every scan) the canary keys are evaluated on the GPU in
 *    the same launches as the scan; a mismatch makes nebgpu_scan return NEBGPU_E_COMPUTE (the hits of that call
 *    must be discarded). The cost is negligible (2 attempts per scan).
 * Safe stop (no TDR / no Windows watchdog reset): every kernel launch is bounded to ~NEBGPU_OPT_MAX_LAUNCH_MS of
 * GPU time (default 20 ms); nebgpu_cancel is honoured between launches, so a scan returns <= ~1 launch after
 * cancel; nebgpu_close waits for the device to finish and frees everything. */
#define NEBGPU_OPT_CANARY_EVERY 1   /* 0 = off, N = every N-th scan call */
#define NEBGPU_OPT_VERIFY_ROWS 2    /* rows checked vs CPU after each table build (0 = off), default 8 */
#define NEBGPU_OPT_MAX_LAUNCH_MS 3  /* target GPU time per launch, 2..200, default 20 */
#define NEBGPU_OPT_KEP1_PATH 4      /* KEP1 attempt kernel (since 0.2.9): 0 auto (default; since 0.3.0 at every new model
                                       of a KEP1 slot both are timed on its first mining launches, alternating, the
                                       faster kept; warp only if >= 2 % faster), 1 dp4a, 2 warp
                                       int8 mma (CC >= 8.0, else dp4a). Env NEBGPU_KEP1_PATH=dp4a|warp|auto overrides
                                       the default at open. */
#define NEBGPU_OPT_CHEAP 5          /* V-cheap cost pre-filter, FORCED for every KEP1 slot: 0 off, 1 on. Overrides env
                                       NEBGPU_CHEAP. Fully computes only nonces with <= NEBGPU_OPT_CHEAPK live sample rows
                                       and skips the rest (bit-exact for the computed ones; the pool re-verifies each
                                       share). Not set (default since 0.6.0): chosen automatically, see NEBGPU_OPT_AUTO. */
#define NEBGPU_OPT_CHEAPK 6         /* V-cheap live-row threshold 0..16 (default 16 = accept all). Overrides NEBGPU_CHEAPK. */
#define NEBGPU_OPT_DTABLE 8         /* Z-dtable: precomputed digest table for the attempts with few live rows. 0 off,
                                       1 on (forced; not set = automatic since 0.6.0, see NEBGPU_OPT_AUTO). Overrides env
                                       NEBGPU_DTABLE. A KEP1 slot whose model suits it
                                       (P(L <= MAXL) >= MINP, a dead row exists, the table fits the memory cap) then
                                       screens every nonce in one fused kernel and takes the digest of each attempt
                                       with <= MAXL live rows from the table (bit-exact); the other nonces are skipped
                                       (or, with FULLK, computed by the full path). Other slots keep their path. */
#define NEBGPU_OPT_DTABLE_MAXL 9    /* Z-dtable: live rows covered by the table 0..3 (default 2 = singletons + pairs;
                                       3 adds the triples of a prefix of live ids that fits NEBGPU_DTABLE_GB). */
#define NEBGPU_OPT_DTABLE_FULLK 10  /* Z-dtable: -1 (default) pure table; MAXL < K <= 16: nonces with MAXL < L <= K run
                                       the full attempt path. Overrides env NEBGPU_DTABLE_FULLK. */
#define NEBGPU_OPT_DTABLE_MINP 11   /* Z-dtable: smallest P(L <= MAXL) of a model that gets a table, in 1e-4 (default
                                       30 = 0.3 % since 0.6.0, 100 = 1 % before; 0 = always; tests). Overrides env
                                       NEBGPU_DTABLE_MINP (a fraction). */
#define NEBGPU_OPT_AUTO 12          /* 0.6.0: automatic path per KEP1 slot and model, 1 on (default), 0 off (the 0.5.2
                                       paths). Overrides env NEBGPU_AUTO. On CC >= 7.5, for every mining slot not forced by
                                       NEBGPU_OPT_CHEAP / NEBGPU_OPT_DTABLE (or their env): the slot's full path is timed
                                       on the first mining launches (kernel trial, then the cheap trial: thresholds k 3..6
                                       and the plain path, k by speed within 3 % of the best, the closest rival per watt
                                       when NVML board energy is available); from the next scan call on, the digest table
                                       is used when the model passes its gate (NEBGPU_OPT_DTABLE_MINP, memory) and its
                                       expected / measured evaluated rate is >= 0.8 x the full path's (the table draws
                                       ~0.6-0.8 x the power). Every path is bit-exact; nebgpu_slot_path tells which one a
                                       slot uses. */
#define NEBGPU_OPT_DTV 13           /* Z-dtable screener variant (tuning/tests; every variant gives the same integers):
                                       0..3 k_dt_scan, >= 4 the S2 screener k_dt_scan2 (see NEBGPU_DTV in nebgpu.cu).
                                       -1 = env NEBGPU_DTV / build default. */
#define NEBGPU_OPT_L3 14            /* L3IE (k-l3ie, experimental): with the digest table at L <= 2, also the attempts with
                                       3 live rows whose inclusion-exclusion correction is cheap (cost <= NEBGPU_OPT_L3_THR),
                                       exact. -1 = env NEBGPU_L3 (not set: since 0.7.0 automatic per model on CC >= 7.5 when
                                       P(L <= 2) < 1.3 %, a short trial decides), 0 off, 1 on. */
#define NEBGPU_OPT_L3_THR 15        /* L3IE cost threshold (ops estimate; < 0: every L = 3 attempt). INT64_MIN = env
                                       NEBGPU_L3_THR (default 30000). */
NEBGPU_API int nebgpu_set_option(nebgpu_ctx *ctx, int option, int64_t value);
/* Z-dtable / V-cheap honest accounting: screened = nonces scanned (as nebgpu_stats attempts); evaluated = attempts whose
 * exact ticket was computed (full attempts + table attempts; each has the same win probability, this is the rate to
 * report as hashrate); table = attempts taken from the digest table. Any pointer may be NULL. Since 0.6.0 (a host must
 * look the symbol up: older engines lack it; there every screened nonce is evaluated). */
NEBGPU_API int nebgpu_eval_stats(const nebgpu_ctx *ctx, uint64_t *screened, uint64_t *evaluated, uint64_t *table);
/* 0.6.0: the path of a dataset slot: 2 the digest table of its model, 1 the cheap filter, 0 the plain path (all
 * bit-exact; 1 and 2 skip nonces, so a scan call of 2^25+ nonces is normal there: ~5-130 ms at 0.25-5 G nonces/s);
 * text (may be NULL) gets a one-line summary ("digest table L<=2 (...)", "cheap k=4 (...)", "plain (...)"). */
NEBGPU_API int nebgpu_slot_path(const nebgpu_ctx *ctx, int ds_index, char *text, size_t len);
/* Z-dtable state of a slot: returns 1 table ready for the slot's model, -1 off for it (text says why), 0 not built;
 * text (may be NULL) gets a one-line summary. */
NEBGPU_API int nebgpu_dtable_info(nebgpu_ctx *ctx, int ds_index, char *text, size_t len);
/* The KEP1 attempt kernel in use (since 0.2.9; chosen per dataset slot in auto mode; since 0.6.0 each slot's text is
 * followed by " | <path>" as nebgpu_slot_path gives it, when there is one): 1 dp4a, 2 warp mma, 3 both (one
 * per slot), 0 not chosen yet (no KEP1 model mined on so far; a slot whose new model is still being timed keeps the
 * kernel of its previous model in the answer until the choice is made). text (may be NULL) gets a line such as
 * "warp-mma (auto: warp-mma 33.61, dp4a 20.95 M/s)" or "mixed per slot: 0 dp4a (auto: ...); 1 warp-mma (auto: ...)". */
NEBGPU_API int nebgpu_kep1_path(const nebgpu_ctx *ctx, char *text, size_t len);
/* Explicit check of the card at the current clocks: n_keys random attempt keys on the model of ds_index (or a
 * built-in synthetic model when ds_index < 0), GPU vs CPU reference. 0 = all equal, NEBGPU_E_COMPUTE = mismatch
 * (*bad gets the count). Cost: n_keys * (2 ms KEP1 / 7 ms RET2) of CPU. */
NEBGPU_API int nebgpu_canary(nebgpu_ctx *ctx, int ds_index, uint32_t n_keys, uint32_t *bad);
/* Quick speed measurement at the current clock point (for auto-lock / auto-uv): scans for ~ms milliseconds on the
 * model of ds_index (or a built-in synthetic model of arch `arch` when ds_index < 0) with an impossible target,
 * steady-state after a short warm-up; *attempts_per_s gets the rate (since 0.6.0: evaluated attempts, see
 * nebgpu_eval_stats; the synthetic slot never uses the automatic paths). Includes the canary check: returns
 * NEBGPU_E_COMPUTE if the card computed wrong during the measurement. Typical ms: 1000-3000. */
NEBGPU_API int nebgpu_bench(nebgpu_ctx *ctx, int ds_index, uint32_t arch, uint32_t ms, double *attempts_per_s);

/* Statistics of the context: attempts since open (since the V-cheap / Z-dtable paths: nonces SCREENED, not all of them
 * evaluated -- see nebgpu_eval_stats for the rate), table builds, ms of the last table build. */
NEBGPU_API int nebgpu_stats(const nebgpu_ctx *ctx, uint64_t *attempts, uint32_t *builds, double *last_build_ms);

/* ---- CPU reference (pow/ref/nebhash2.c, compiled in; no GPU needed) ---- */
/* Ticket of one nonce of a header on a model and dataset (records as in nebgpu_load_dataset). Also the
 * digest and the indices when the pointers are not NULL. ~2 ms (KEP1) / ~7 ms (RET2) of one core. */
NEBGPU_API int nebgpu_ref_ticket(uint32_t arch, const uint8_t *records, uint32_t n, const int8_t *weights,
                                 const uint8_t header[80], uint32_t nonce, uint8_t ticket[32], uint8_t digest[32],
                                 uint32_t idx[16]);
/* Unpack Model-frame weights; returns the weight count or a negative error. out must hold 26000 entries. */
NEBGPU_API int nebgpu_unpack_weights(uint32_t arch, const uint8_t *packed, size_t len, int8_t *out);
/* weights_hash of spec nebhash-v2 section 6 (to check the Model frame's hash). */
NEBGPU_API int nebgpu_weights_hash(uint32_t arch, const int8_t *weights, uint8_t out[32]);
/* 1 if ticket <= target as 256-bit little-endian integers, else 0. */
NEBGPU_API int nebgpu_ticket_le(const uint8_t ticket[32], const uint8_t target[32]);

#ifdef __cplusplus
}
#endif
#endif
