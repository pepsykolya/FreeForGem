// FreeForGemPear — see LICENSE
//! NOID proof of work on the CPU with the official crate (`noid_poseidon2b`, Apache-2.0): the reference every GPU result is
//! checked against (self-test, tuning stress check, every hit before it is submitted).
//!
//! Digest = Poseidon2b sponge (capacity IV of the tag `POWHDR__`) over the 16 PoW fields (8 `absorb_pair`, `finalize_no_pad`),
//! 32 bytes LE; a share / block is valid when digest < target as 256-bit little-endian numbers (equality fails).
use noid_core::Block128;
use noid_poseidon2b::batch::FixedFieldNonceBatch;
use noid_poseidon2b::native::compression::Poseidon2bSponge;
use noid_poseidon2b::native::domain::{TAG_POWHDR, capacity_iv};

pub const NONCE_FIELD: usize = 10;

pub fn fields_of(raw: &[u8; 256]) -> [u128; 16] {
    let mut f = [0u128; 16];
    for (i, x) in f.iter_mut().enumerate() {
        *x = u128::from_le_bytes(raw[16 * i..16 * i + 16].try_into().unwrap());
    }
    f
}

/// Digest of one complete set of fields (the nonce is field 10).
pub fn digest(raw: &[u8; 256]) -> [u8; 32] {
    let f = fields_of(raw);
    let mut s = Poseidon2bSponge::with_iv(capacity_iv(TAG_POWHDR));
    for c in f.chunks_exact(2) {
        s.absorb_pair(Block128(c[0]), Block128(c[1]));
    }
    s.finalize_no_pad()
}

/// digest < target, both 256-bit little-endian.
pub fn below(d: &[u8; 32], t: &[u8; 32]) -> bool {
    for i in (0..32).rev() {
        if d[i] != t[i] {
            return d[i] < t[i];
        }
    }
    false
}

/// Bytes 24..31 as u64 LE: what the GPU kernel compares (candidate filter).
pub fn hi64(d: &[u8; 32]) -> u64 {
    u64::from_le_bytes(d[24..32].try_into().unwrap())
}

/// Digests of the nonces `nonce0 .. nonce0 + n` (field 10 as u128) over all CPU threads (official batch kernel).
pub fn digests(raw: &[u8; 256], nonce0: u128, n: usize) -> Vec<[u8; 32]> {
    let f: Vec<Block128> = fields_of(raw).iter().map(|&x| Block128(x)).collect();
    let mut out = vec![[0u8; 32]; n];
    let threads = std::thread::available_parallelism().map_or(4, |x| x.get()).clamp(1, 32);
    let chunk = n.div_ceil(threads).max(4096);
    std::thread::scope(|s| {
        for (k, part) in out.chunks_mut(chunk).enumerate() {
            let f = &f;
            s.spawn(move || {
                let mut b = FixedFieldNonceBatch::new(TAG_POWHDR, f, NONCE_FIELD);
                for (j, sub) in part.chunks_mut(4096).enumerate() {
                    b.hash_into(nonce0 + (k * chunk + j * 4096) as u128, sub);
                }
            });
        }
    });
    out
}

/// The 16 test vectors generated with the official crate (fields, digest), hex.
pub const VECTORS: [(&str, &str); 16] = include!("vectors.inc");

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(i: usize) -> [u8; 256] {
        crate::stratum::hex_decode(VECTORS[i].0).unwrap().try_into().unwrap()
    }

    #[test]
    fn official_vectors() {
        for (i, (_, d)) in VECTORS.iter().enumerate() {
            let r = raw(i);
            let h: String = digest(&r).iter().map(|x| format!("{x:02x}")).collect();
            assert_eq!(&h, d, "vector {i}");
            let n0 = u128::from_le_bytes(r[160..176].try_into().unwrap());
            assert_eq!(digests(&r, n0, 1)[0], digest(&r), "batch, vector {i}");
        }
    }

    #[test]
    fn batch_equals_single() {
        let mut r = raw(3);
        let n0 = u128::from_le_bytes(r[160..176].try_into().unwrap());
        let t = std::time::Instant::now();
        let b = digests(&r, n0, 376_832);
        eprintln!("376832 digests on the CPU: {:?}", t.elapsed());
        for k in [0usize, 1, 4095, 4096, 100_000, 376_831] {
            r[160..176].copy_from_slice(&(n0 + k as u128).to_le_bytes());
            assert_eq!(b[k], digest(&r), "nonce +{k}");
        }
    }
}
