// FreeForGemPear — see LICENSE
// pearlgpu.h -- C ABI of the Pearl search library (CUDA, sm_86+).
//
// Matrices (fixed by the host's mining configuration):
//   A (m x k) signal = 64 everywhere except row 0 (the attempt nonce): rows 0..7 of row block 0 are wrong on the GPU,
//                    so a hit whose tile contains row 0 must be dropped (the host verifies every hit anyway).
//   B (k x n) signal = 64 everywhere, fixed per job.
//   noise: A' = A + A_L * P_A, B' = B + P_B * B_R exactly as in zk-pow/src/circuit/pearl_noise.rs.
// Inner-hash tile: rows {0,8,...,56} x cols {0,1,8,9,...,56,57}; a hit reports the tile origin (t_rows, t_cols).
// Requirements: n % 256 == 0, k == 2048, rank == 128, m % 128 == 0 (kernel v2.2, pearl_gen13_sm86.cuh).
#pragma once
#include <stdint.h>

#ifdef _WIN32
#define PG_API __declspec(dllexport)
#elif defined(__GNUC__)
#define PG_API __attribute__((visibility("default")))  // the release library is built with -fvisibility=hidden
#else
#define PG_API
#endif

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
  uint32_t row;         // t_rows
  uint32_t col;         // t_cols
  uint32_t jackpot[8];  // BLAKE3(transcript, key = a_noise_seed), little-endian words
} pg_hit;

// Library state is per calling thread: a thread that calls pg_init(d) owns device d (its own B buffer, slots and
// error string); use one thread per GPU for multi-GPU mining.
// Number of CUDA devices visible to the process (honours CUDA_VISIBLE_DEVICES); name of device `device`.
PG_API int pg_device_count(void);
// Common dimension k the library (and its embedded kernel) was built for.
PG_API int pg_kdim(void);
PG_API int pg_device_name(int device, char* out, int len);

// Select the CUDA device (ordinal as seen by the process). Returns 0 on success.
PG_API int pg_init(int device);

// Set the job's B side. br_t: n x 128 values of B_R (column j of B_R as row j, int8 in [-32,31]);
// q1, q2: the k index pairs of P_B (first/second index of each column of B_L^T, as generate_permutation_matrix).
PG_API int pg_set_job(const int8_t* br_t, const int32_t* q1, const int32_t* q2, int n, int k);

// One attempt: A_L is generated on the GPU from a_noise_seed (label "A_tensor"), p1/p2 are P_A's index pairs.
// key = a_noise_seed as 8 LE words, target = final share target (LE words, word 7 most significant).
// Writes up to max_hits hits, returns the number of hits found (may exceed max_hits), negative on error.
PG_API int pg_attempt(const uint8_t a_noise_seed[32], const int32_t* p1, const int32_t* p2, int m, const uint32_t target[8],
               pg_hit* hits, int max_hits);

// Asynchronous form of pg_attempt with two slots (0, 1): pg_launch enqueues the attempt and returns at once,
// pg_wait(slot) blocks until it finished and returns the hit count (copies at most min(max_hits, 64) hits).
// Launch attempt i+1 into the other slot before waiting for attempt i to keep the GPU busy.
PG_API int pg_launch(int slot, const uint8_t a_noise_seed[32], const int32_t* p1, const int32_t* p2, int m,
              const uint32_t target[8]);
PG_API int pg_wait(int slot, pg_hit* hits, int max_hits);

// Debug: copy the GPU-generated A_L rows [row0, row0 + rows) as int8 (rows x 128) for comparison with the reference.
PG_API int pg_debug_al(const uint8_t a_noise_seed[32], int m, int row0, int rows, int8_t* out);

PG_API const char* pg_last_error(void);

#ifdef __cplusplus
}
#endif
