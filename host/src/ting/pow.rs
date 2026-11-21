// FFGT — see LICENSE
//! Ting proof of work on the CPU (the reference every GPU candidate is checked against before it is submitted):
//!   BlockId = SHA-256("Ting/" || u16le(len "block") || "block" || header[294])   (node/wiki/data/encoding.md, H(tag, x))
//! valid when BlockId, read as a big-endian number, is at most the target (consensus valid_pow).  The nonce is the header's
//! last field: 16 bytes, little-endian, at offset 278.  The GPU library fixes the nonce bytes 0..9 and 14..15 per call (the
//! `base`) and enumerates g in bytes 10..13, big-endian.
use sha2::{Digest, Sha256};

pub const HEADER_LEN: usize = 294;
pub const NONCE_OFFSET: usize = 278;
pub const PREFIX: &[u8; 12] = b"Ting/\x05\x00block";

/// BlockId of `header` with `nonce` (16 little-endian bytes) in place.
pub fn block_id(header: &[u8; HEADER_LEN], nonce: &[u8; 16]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(PREFIX);
    h.update(&header[..NONCE_OFFSET]);
    h.update(nonce);
    h.finalize().into()
}

/// `id <= target`, both big-endian 32-byte numbers.
pub fn meets(id: &[u8; 32], target: &[u8; 32]) -> bool {
    id <= target
}

/// The nonce the library tries for `g`: `base` with bytes 10..13 = g big-endian.
pub fn nonce_of(base: &[u8; 16], g: u32) -> [u8; 16] {
    nonce_of_w(base, 11, g)
}

/// The nonce for g in message word `word` (9: bytes 2..5, 10: bytes 6..9, 11: bytes 10..13), big-endian.
pub fn nonce_of_w(base: &[u8; 16], word: i32, g: u32) -> [u8; 16] {
    let mut n = *base;
    let b = (2 + 4 * (word - 9)) as usize;
    n[b..b + 4].copy_from_slice(&g.to_be_bytes());
    n
}

pub fn nonce_u128(n: &[u8; 16]) -> u128 {
    u128::from_le_bytes(*n)
}

/// 2^256 / (target + 1): the expected number of hashes per block (as f64; enough for display and ETA).
pub fn expected_hashes(target: &[u8; 32]) -> f64 {
    let mut t = 0f64;
    for &b in target.iter() {
        t = t * 256.0 + b as f64;
    }
    2f64.powi(256) / (t + 1.0)
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// The library's job as this host computes it (midstate of message bytes 0..255, last block, target words), to check the
/// library's own job build once at start.
pub fn job_words(header: &[u8; HEADER_LEN], target: &[u8; 32]) -> super::gpu::TgJob {
    let mut m = [0u8; 320];
    m[..12].copy_from_slice(PREFIX);
    m[12..12 + HEADER_LEN].copy_from_slice(header);
    m[12 + NONCE_OFFSET..12 + HEADER_LEN].fill(0);
    m[306] = 0x80;
    m[312..].copy_from_slice(&(306u64 * 8).to_be_bytes());
    let mut st: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    for b in 0..4 {
        let block: [u8; 64] = m[64 * b..64 * b + 64].try_into().unwrap();
        sha2::compress256(&mut st, &[block.into()]);
    }
    let w = |o: usize| u32::from_be_bytes(m[o..o + 4].try_into().unwrap());
    let mut j = super::gpu::TgJob { midstate: st, ..Default::default() };
    for i in 0..16 {
        j.tail[i] = w(256 + 4 * i);
    }
    for i in 0..8 {
        j.target[i] = u32::from_be_bytes(target[4 * i..4 * i + 4].try_into().unwrap());
    }
    j
}

/// BlockId of a library job with `nonce` in place (the last SHA-256 block from the job's midstate): the CPU reference of the
/// test engine of iso.rs; equal to `block_id` of the header the job was built from.
#[cfg_attr(not(test), allow(dead_code))]
pub fn job_hash(j: &super::gpu::TgJob, nonce: &[u8; 16]) -> [u8; 32] {
    let mut block = [0u8; 64];
    for (i, w) in j.tail.iter().enumerate() {
        block[4 * i..4 * i + 4].copy_from_slice(&w.to_be_bytes());
    }
    // message bytes 290..305 = the nonce: offsets 34..49 of the last block (it starts at message byte 256)
    block[12 + NONCE_OFFSET - 256..12 + NONCE_OFFSET - 256 + 16].copy_from_slice(nonce);
    let mut st = j.midstate;
    sha2::compress256(&mut st, &[block.into()]);
    let mut id = [0u8; 32];
    for (i, w) in st.iter().enumerate() {
        id[4 * i..4 * i + 4].copy_from_slice(&w.to_be_bytes());
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_hash_equals_block_id() {
        let h: [u8; HEADER_LEN] = unhex(VEC).unwrap().try_into().unwrap();
        let j = job_words(&h, &[0xff; 32]);
        for n in [0u8, 4, 77, 255] {
            let mut nonce = [n; 16];
            nonce[3] ^= 0x5a;
            assert_eq!(job_hash(&j, &nonce), block_id(&h, &nonce));
        }
    }
    // conformance/vectors/v3-headers/an-empty-block.json (valid; its nonce is 4) and proof-of-work-above-the-target.json
    // (the same header, nonce 0: invalid); the development target 3fff..ff
    const VEC: &str = "01008578787146be1ef0aacc797754001290272915e89cd9e5784fe1368e4c2131bc0d000000000000003f225a70abe46ef29f09504d0b69ce5e3d550e4d86bb253dc6cd163b5217989d0cf45365000000003fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8c2beadecc1824b6e7f88cd39fff6d8ffdc9e8a41e93254d7f7109ba7d218f49de3aa3b4c7df30035eb3ddc2d357d2a54521c27a4742438a9ab4561cb516ee6b572d8e192ec138fe522c0aefb5fc2f4a42e2f6c7acdbe887071e5ca50eea4e83000000000000000000000000000000002e0100000000000000000000000000000000000051976778d26056c36803109ea49beaec0c618c49fc39c63fb1f10e51bf56592c04000000000000000000000000000000";

    #[test]
    fn conformance_vector_first_valid_nonce_is_4() {
        let h: [u8; HEADER_LEN] = unhex(VEC).unwrap().try_into().unwrap();
        let mut t = [0xffu8; 32];
        t[0] = 0x3f;
        let first = (0u8..64).find(|&n| {
            let mut nonce = [0u8; 16];
            nonce[0] = n;
            meets(&block_id(&h, &nonce), &t)
        });
        assert_eq!(first, Some(4));
        assert_eq!(u128::from_le_bytes(h[NONCE_OFFSET..].try_into().unwrap()), 4);
    }

    #[test]
    fn nonce_layout() {
        let base = [7u8; 16];
        let n = nonce_of(&base, 0x0102_0304);
        assert_eq!(&n[10..14], &[1, 2, 3, 4]);
        assert_eq!(n[9], 7);
        assert_eq!(n[14], 7);
    }
}
