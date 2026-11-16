// FreeForGem — see LICENSE
//! BLAKE2b-256 (RFC 7693, unkeyed) on the CPU: the independent check of every PROX candidate before it is submitted (the GPU
//! library is never trusted), the self-test reference and the work rules of the Proxima transaction-hash PoW (library upgrade 6):
//!   h = BLAKE2b-256(message || nonce as 8 bytes big-endian);  work = trailing zero bits of h[24..32] read big-endian;
//!   work key = h[24..32] || h[6..24] (26 bytes, smaller is better; solo: a solution must be below the node's `beat`).
const IV: [u64; 8] = [
    0x6a09e667f3bcc908, 0xbb67ae8584caa73b, 0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1, 0x510e527fade682d1, 0x9b05688c2b3e6c1f, 0x1f83d9abfb41bd6b, 0x5be0cd19137e2179,
];
const SIGMA: [[usize; 16]; 12] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
];

#[inline(always)]
fn g(v: &mut [u64; 16], a: usize, b: usize, c: usize, d: usize, x: u64, y: u64) {
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
    v[d] = (v[d] ^ v[a]).rotate_right(32);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(24);
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(63);
}

fn compress(h: &mut [u64; 8], block: &[u8; 128], t: u128, last: bool) {
    let mut m = [0u64; 16];
    for (i, w) in m.iter_mut().enumerate() {
        *w = u64::from_le_bytes(block[8 * i..8 * i + 8].try_into().unwrap());
    }
    let mut v = [0u64; 16];
    v[..8].copy_from_slice(h);
    v[8..].copy_from_slice(&IV);
    v[12] ^= t as u64;
    v[13] ^= (t >> 64) as u64;
    if last {
        v[14] = !v[14];
    }
    for s in &SIGMA {
        g(&mut v, 0, 4, 8, 12, m[s[0]], m[s[1]]);
        g(&mut v, 1, 5, 9, 13, m[s[2]], m[s[3]]);
        g(&mut v, 2, 6, 10, 14, m[s[4]], m[s[5]]);
        g(&mut v, 3, 7, 11, 15, m[s[6]], m[s[7]]);
        g(&mut v, 0, 5, 10, 15, m[s[8]], m[s[9]]);
        g(&mut v, 1, 6, 11, 12, m[s[10]], m[s[11]]);
        g(&mut v, 2, 7, 8, 13, m[s[12]], m[s[13]]);
        g(&mut v, 3, 4, 9, 14, m[s[14]], m[s[15]]);
    }
    for i in 0..8 {
        h[i] ^= v[i] ^ v[i + 8];
    }
}

/// BLAKE2b with a 32-byte digest of `parts` concatenated.
pub fn hash256(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = IV;
    h[0] ^= 0x0101_0000 ^ 32;
    let total: usize = parts.iter().map(|p| p.len()).sum();
    let mut buf = [0u8; 128];
    let (mut fill, mut done) = (0usize, 0usize);
    for p in parts {
        for &b in *p {
            if fill == 128 {
                done += 128;
                compress(&mut h, &buf, done as u128, false);
                fill = 0;
            }
            buf[fill] = b;
            fill += 1;
        }
    }
    buf[fill..].fill(0);
    compress(&mut h, &buf, total as u128, true);
    let mut out = [0u8; 32];
    for i in 0..4 {
        out[8 * i..8 * i + 8].copy_from_slice(&h[i].to_le_bytes());
    }
    out
}

/// h = BLAKE2b-256(message || nonce_be8)
pub fn hash_nonce(msg: &[u8], nonce: u64) -> [u8; 32] {
    hash256(&[msg, &nonce.to_be_bytes()])
}

/// Work of a hash: trailing zero bits of h[24..32] read big-endian (64 when all zero).
pub fn work_bits(h: &[u8; 32]) -> u32 {
    u64::from_be_bytes(h[24..32].try_into().unwrap()).trailing_zeros()
}

/// Work key h[24..32] || h[6..24]: smaller is better.
pub fn work_key(h: &[u8; 32]) -> [u8; 26] {
    let mut k = [0u8; 26];
    k[..8].copy_from_slice(&h[24..32]);
    k[8..].copy_from_slice(&h[6..24]);
    k
}

/// Node vectors (hashVectors of the Proxima node, MIT): message bytes 0,1,2.. of length n, nonce 0x0102030405060708.
pub const NODE_VECTORS: &[(usize, &str)] = &[
    (100, "b7af2281301d649b893e3673679fdf7e61d74dd7347365c17518dc69253506c8"),
    (119, "a3687092de02840d3e8c30e5025d320ac20cc1feb342ba1fba78c60c41c07d3c"),
    (120, "bae38c5df1e42849a52915f23f758ec092485b76e3e5c8ec4e6204c2bef1e6ca"),
    (127, "a1443e852985373cdba3b004be8ab05e20610b869f8389f919b01f537e0c9646"),
    (128, "aaf45eb2daad328c85f92c61f87154454b369f6fbb7c6bffff43d1a305518259"),
    (129, "3f8e4f06a0c13899d168aeb96a93e857a65141a0cf4dee9748324029ecccf17c"),
    (200, "82eba8169dd58a9e6042487be103689d728a36cff1035e2ab1da07196dec3d3d"),
    (247, "20b9eef755e709fd11307bffc52e773d05c058a63134f65166af9c1744d8febc"),
    (248, "ddddc25892c55fe25c48c9660493f7830aa7ae43f29efabcfce2a329c71e2583"),
    (256, "307a1a5b84ff17649608b11a3c701f04ef39548b42e9c432ae1d889fbc56b296"),
    (300, "3a8a0f43b3c3d96fe5466a2f1c751ff06308d08ed409b5a5f4d7d069ef14c604"),
];
/// Pool vectors (txid-blake2b, public pool protocol vectors): prefix[i] = (i*7+3) & 0xff, nonce 0x0102 || 030405060708;
/// (prefix_len, hash, work, first extranonce2 counting up from 0 with >= 12 zero bits).
pub const POOL_VECTORS: &[(usize, &str, u32, &str)] = &[
    (112, "179ad8f3e27dbb840cd3431294e9a2f73c7cfce68ffc04932e27abb51ba07cc1", 0, "000000000350"),
    (119, "23a9821464e185ca4df71624cb7b434bc72cbcf630bedab34e9a25d59ecb91f6", 1, "0000000009c6"),
    (120, "4778e9ff057e689d7aee7144b9175e717c3d985f8233917541232fdb75a55023", 0, "000000000235"),
    (121, "5ec330b79c9c4ed06b0b6cf10576a894c58db00a01393750881f0102955e710f", 0, "000000002de1"),
    (128, "27ed09864fa792b64db6d36de8201a89697bf8684c518cd32885ef0ed1b7942b", 0, "000000001fc1"),
    (247, "29380018b185cf6dd857527e73909346313b5f73f83c77cc8342488a6a69ac7b", 0, "000000000c51"),
    (248, "39a22194d7350d6f401628dc3a31792204dc12d316e923d76c142c28da250a1c", 2, "000000002e78"),
    (600, "56543577bce2ecc81ab261b2b5bc841941c78e6307af6c636aa36377b7cf641b", 0, "0000000027a1"),
    (632, "45ced55f04f3140fc6d2f574259e66bb99b0ba3dcd14f19bb51e109b9be461c1", 0, "00000000094f"),
];

#[cfg(test)]
mod tests {
    use super::*;
    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
    #[test]
    fn rfc7693_abc_512_prefix() {
        // BLAKE2b-256("") known value
        assert_eq!(hex(&hash256(&[b""])), "0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8");
        assert_eq!(hex(&hash256(&[b"abc"])), "bddd813c634239723171ef3fee98579b94964e3bb1cb3e427262c8c068d52319");
    }
    #[test]
    fn node_and_pool_vectors() {
        for &(n, want) in NODE_VECTORS {
            let m: Vec<u8> = (0..n).map(|i| i as u8).collect();
            assert_eq!(hex(&hash_nonce(&m, 0x0102030405060708)), want, "node len {n}");
        }
        for &(n, want, work, first) in POOL_VECTORS {
            let p: Vec<u8> = (0..n).map(|i| ((i * 7 + 3) & 0xff) as u8).collect();
            let h = hash_nonce(&p, 0x0102030405060708);
            assert_eq!(hex(&h), want);
            assert_eq!(work_bits(&h), work);
            let e = (0u64..).find(|e| work_bits(&hash_nonce(&p, 0x0102_0000_0000_0000 | e)) >= 12).unwrap();
            assert_eq!(format!("{e:012x}"), first);
        }
    }
}
