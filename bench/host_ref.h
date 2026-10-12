// FreeForGemPear — see LICENSE
// host_ref.h -- CPU reference pieces shared by the benches (BLAKE3 single block, transcript of one tile)
#pragma once
#include <cstdint>
#include <cstring>
#include <vector>
#include <random>

// ---- host BLAKE3 single block keyed ----
inline uint32_t hrotr(uint32_t x, int n) { return (x >> n) | (x << (32 - n)); }
inline void hg(uint32_t& a, uint32_t& b, uint32_t& c, uint32_t& d, uint32_t x, uint32_t y) {
  a = a + b + x; d = hrotr(d ^ a, 16); c = c + d; b = hrotr(b ^ c, 12);
  a = a + b + y; d = hrotr(d ^ a, 8);  c = c + d; b = hrotr(b ^ c, 7);
}
inline void host_blake3_keyed_64(const uint32_t* key, const uint32_t* msg, uint32_t* out) {
  uint32_t v[16] = {key[0], key[1], key[2], key[3], key[4], key[5], key[6], key[7],
                    0x6A09E667u, 0xBB67AE85u, 0x3C6EF372u, 0xA54FF53Au, 0, 0, 64, 27};
  uint32_t m[16]; memcpy(m, msg, 64);
  static const int P[16] = {2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8};
  for (int r = 0; r < 7; ++r) {
    hg(v[0], v[4], v[8], v[12], m[0], m[1]);   hg(v[1], v[5], v[9], v[13], m[2], m[3]);
    hg(v[2], v[6], v[10], v[14], m[4], m[5]);  hg(v[3], v[7], v[11], v[15], m[6], m[7]);
    hg(v[0], v[5], v[10], v[15], m[8], m[9]);  hg(v[1], v[6], v[11], v[12], m[10], m[11]);
    hg(v[2], v[7], v[8], v[13], m[12], m[13]); hg(v[3], v[4], v[9], v[14], m[14], m[15]);
    uint32_t t[16]; for (int i = 0; i < 16; ++i) t[i] = m[P[i]]; memcpy(m, t, 64);
  }
  for (int i = 0; i < 8; ++i) out[i] = v[i] ^ v[i + 8];
}

inline void fill_random(std::vector<int8_t>& v, uint64_t seed) {
  std::mt19937_64 r(seed);
  for (auto& x : v) x = (int8_t)((int)(r() % 255) - 127);  // [-127, 127] like A + noise
}

// CPU transcript for tile at (row origin r0 = 64*a + g, col origin c0 = 64*b + 2t):
// rows r0 + 8i (i<8), cols c0 + {0,1} + 8j (j<8)
inline void cpu_transcript(const std::vector<int8_t>& A, const std::vector<int8_t>& Bt, int k, int r0, int c0,
                           uint32_t* jp) {
  int32_t acc[8][16] = {};
  memset(jp, 0, 64);
  for (int ll = 128; ll <= k; ll += 128) {
    for (int i = 0; i < 8; ++i)
      for (int j = 0; j < 16; ++j) {
        const int row = r0 + 8 * i, col = c0 + (j & 1) + 8 * (j >> 1);
        const int8_t* a = &A[(size_t)row * k];
        const int8_t* b = &Bt[(size_t)col * k];
        int32_t s = acc[i][j];
        for (int l = ll - 128; l < ll; ++l) s += (int32_t)a[l] * (int32_t)b[l];
        acc[i][j] = s;
      }
    uint32_t x = 0;
    for (int i = 0; i < 8; ++i)
      for (int j = 0; j < 16; ++j) x ^= (uint32_t)acc[i][j];
    const int tid = (ll / 128 - 1) % 16;
    jp[tid] = ((jp[tid] << 13) | (jp[tid] >> 19)) ^ x;
  }
}

