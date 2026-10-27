// FreeForGemPear — see LICENSE
//! Job preparation, incremental commitments, noise inputs for the GPU, proof building and CPU verification.
//! Uses the official zk-pow / pearl-blake3 crates for everything consensus-relevant.
use anyhow::{Context, Result, ensure};
use blake3::hazmat::{HasherExt, Mode, merge_subtrees_non_root, merge_subtrees_root};
use pearl_blake3::{MerkleTree, blake3_digest};
use primitive_types::U256;
use zk_pow::api::proof::{IncompleteBlockHeader, MMAType, MiningConfiguration, PeriodicPattern, PublicProofParams, SeedDerivation};
use zk_pow::api::proof_utils::{CompiledPublicParams, compute_jackpot_hash};
use zk_pow::api::verify::verify_plain_proof;
use zk_pow::circuit::chip::compute_jackpot;
use zk_pow::circuit::pearl_noise::{compute_noise, generate_permutation_matrix, generate_uniform_random_matrix, get_random_hash, mul_hi_u32};
use zk_pow::ffi::plain_proof::{CertificateVersion, MatrixMerkleProof, PlainProof};

pub const N: usize = 512;
/// Default rows of A per attempt (display / self-test / fallback); overridable with `FFGP_M`.
const DEFAULT_M: usize = 47104;

/// `FFGP_M` if set and valid (a multiple of 128 in 1024..=2^20), else None.  Panics on an invalid value.
fn ffgp_m_override() -> Option<usize> {
    static MV: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    *MV.get_or_init(|| match std::env::var("FFGP_M") {
        Ok(v) => match v.trim().parse::<usize>() {
            Ok(x) if x % 128 == 0 && (1024..=1_048_576).contains(&x) => Some(x),
            _ => panic!("FFGP_M={v}: want a multiple of 128 in 1024..=1048576"),
        },
        Err(_) => None,
    })
}

/// Default rows of A per attempt: `FFGP_M` or DEFAULT_M.  Used for the banner, the rate display and the
/// self-test; each mining device uses `device_m` for its own SM count.
pub fn m() -> usize {
    ffgp_m_override().unwrap_or(DEFAULT_M)
}

/// Rows of A for a device with `sm` streaming multiprocessors.  Larger m amortizes the per-attempt prologue
/// (P_A build, pipeline fill, jackpot epilogue) over more row-blocks, at the cost of host RAM (m*k bytes of
/// `a_base` per job) and a larger Merkle tree per proof.  Measured on a 3070 (k = 8192, same watts): r = 8 ->
/// 70.57, r = 12 -> 70.67 (+0.14%), r = 16 -> 70.71 (+0.20%) TMAC/s, saturating above that.  The library
/// launches a persistent grid of min(m/128, SM) CTAs and the row-blocks divide evenly only when
/// (m/128) % grid == 0, i.e. m = 128 * SM * r; r is the largest of 16,12,8,4,2 that keeps a_base within
/// 768 MiB.  All five r are even, so m is always a multiple of 256: the Ada kernel t256 (CTA 256 rows, `gpu::row_align() == 256`)
/// keeps its even split of m/256 = SM * r/2 blocks over the SM CTAs (an odd r would leave a wave tail or drop to gen13).
/// `FFGP_M` overrides it (same m on every device).  A mixed-SM rig gets the right m per device
/// (since 1.4.7): each SM group mines its own Job.
pub fn device_m(sm: usize) -> usize {
    if let Some(v) = ffgp_m_override() {
        return v;
    }
    let cap = (768 * 1024 * 1024) / k(); // a_base (m * k bytes per job) budget
    for r in [16usize, 12, 8, 4, 2] {
        let v = 128 * sm * r;
        if sm > 0 && v <= cap {
            return v;
        }
    }
    DEFAULT_M
}

/// Warns when `m` is not a multiple of the row granularity the library chose for this device (`FFGP_M` override only; `device_m`
/// itself is always a multiple of 256): the library then runs those attempts on gen13 instead of the selected kernel.
pub fn warn_m_align(dev: i32, m: usize, align: usize) {
    if align > 0 && m % align != 0 {
        eprintln!("!! GPU {dev}: m={m} is not a multiple of {align} (row granularity of the selected kernel): attempts fall back to the slower kernel; use FFGP_M = multiple of {align}");
    }
}

#[cfg(test)]
mod const_cv_tests {
    use super::*;
    #[test]
    fn piecewise_equals_whole() {
        // the piecewise CV (small buffer) equals the CV of the whole constant slice, for power-of-two and other lengths
        let key = [7u8; 32];
        for (start, len) in [(8usize, 8usize), (16, 16), (64, 40), (128, 128), (1024, 1000), (2048, 1536)] {
            let data = vec![64u8; len * CHUNK];
            assert_eq!(const_cv(&key, start, len, 4), subtree_cv(&key, &data, start), "start {start} len {len}");
        }
    }
}

#[cfg(test)]
mod m_tests {
    #[test]
    fn device_m_is_256_aligned_and_wave_even() {
        if std::env::var_os("FFGP_M").is_some() { return; }
        for sm in 1..=200usize {
            let m = super::device_m(sm);
            assert_eq!(m % 256, 0, "sm {sm} m {m}");
            if m != super::DEFAULT_M { assert_eq!((m / 256) % sm, 0, "sm {sm} m {m}: blocks must split evenly over the SM CTAs"); }
        }
    }
}
/// Common dimension k: the k the loaded libpearlgpu was built for (production 8192), overridable with `PEARL_K` for
/// tests (pg_set_job refuses a mismatch).  Everything else (config, adjustment factor, commitment, proof size) follows
/// from it.
pub fn k() -> usize {
    static KV: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *KV.get_or_init(|| match std::env::var("PEARL_K") {
        Ok(v) => match v.trim().parse::<usize>() {
            Ok(x) if x % 1024 == 0 && x >= 2048 && x <= 8192 => x,
            _ => panic!("PEARL_K={v}: want a multiple of 1024 in 2048..=8192"),
        },
        Err(_) => crate::gpu::kdim(),
    })
}
pub const RANK: usize = 128;
/// rows*cols*(k/rank)*128 = 8*16*(k/RANK)*128 (524288 at k = 4096, 1048576 at 8192); scales a "base" share target to the jackpot target.
pub fn adjustment_factor() -> u64 {
    (8 * 16 * (k() / RANK) * 128) as u64
}
const CHUNK: usize = 1024;

pub const LABEL_A: [u8; 32] = label(*b"A_tensor");
pub const LABEL_B: [u8; 32] = label(*b"B_tensor");
const fn label(l: [u8; 8]) -> [u8; 32] {
    let mut r = [0u8; 32];
    let mut i = 0;
    while i < 8 {
        r[i] = l[i];
        i += 1;
    }
    r
}

pub fn mining_config() -> MiningConfiguration {
    MiningConfiguration {
        common_dim: k() as u32,
        rank: RANK as u16,
        mma_type: MMAType::Int7xInt7ToInt32,
        rows_pattern: PeriodicPattern::from_list(&[0, 8, 16, 24, 32, 40, 48, 56]).unwrap(),
        cols_pattern: PeriodicPattern::from_list(&[0, 1, 8, 9, 16, 17, 24, 25, 32, 33, 40, 41, 48, 49, 56, 57]).unwrap(),
        moe: None,
    }
}

/// Official sanity check of the (config, m, n) combination.
pub fn config_sanity(header: IncompleteBlockHeader, deriv: SeedDerivation, m: usize) -> Result<()> {
    let p = PublicProofParams {
        block_header: header,
        seed_derivation: deriv,
        mining_config: mining_config(),
        hash_a: [0; 32],
        hash_b: [0; 32],
        hash_jackpot: [0; 32],
        m: m as u32,
        n: N as u32,
        t_rows: 0,
        t_cols: 0,
        moe: None,
    };
    p.sanity_check()
}

/// Final jackpot target for pg_attempt as 8 LE words (word 7 most significant).
pub fn target_words(t: U256) -> [u32; 8] {
    let mut b = [0u8; 32];
    t.to_little_endian(&mut b);
    std::array::from_fn(|i| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap()))
}

fn subtree_cv(key: &[u8; 32], data: &[u8], start_chunk: usize) -> [u8; 32] {
    let mut h = blake3::Hasher::new_keyed(key);
    h.set_input_offset((start_chunk * CHUNK) as u64);
    if data.len() >= (1 << 20) {
        h.update_rayon(data); // per-job subtrees
    } else {
        h.update(data); // per-attempt chunks: no thread-pool round trip (the box also runs a CPU miner)
    }
    h.finalize_non_root()
}

/// A buffer of 64-bytes (the value of every A element outside row 0), reused to hash constant subtrees of A without the whole
/// matrix in memory.
const CONST_BUF: usize = 16 << 20;
fn const_buf() -> &'static [u8] {
    static B: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    B.get_or_init(|| vec![64u8; CONST_BUF])
}

/// Keyed BLAKE3 chaining value of the subtree of `len` chunks at chunk `start` whose bytes are all 64 (1.7.3: computed piecewise
/// from a 16 MB buffer, the same tree BLAKE3 builds: left part = the largest power of two below `len`).
fn const_cv(key: &[u8; 32], start: usize, len: usize, buf_chunks: usize) -> [u8; 32] {
    if len <= buf_chunks {
        return subtree_cv(key, &const_buf()[..len * CHUNK], start);
    }
    let left = 1usize << (usize::BITS - 1 - (len - 1).leading_zeros());
    let l = const_cv(key, start, left, buf_chunks);
    let r = const_cv(key, start + left, len - left, buf_chunks);
    merge_subtrees_non_root(&l, &r, Mode::KeyedHash(key))
}

/// The whole A of a job (row 0 = `row0`, every other byte 64), built only while a proof needs it: one at a time in the process
/// (1.7.3: the job no longer keeps a 0.7-0.8 GB copy, and a proof no longer clones it: Rig05, 4x CMP 50HX, 3.8 GB RAM).
pub struct AMatrix {
    pub data: Vec<u8>,
}
/// Proofs building their A right now, and the wake-up for the ones waiting.  More than one at a time only while the free RAM
/// covers another (half of MemAvailable >= one A): a burst of GPUs hitting together (0.6 s per proof on an RTX 3070) does not
/// queue for seconds on a rig with RAM, and a 4 GB rig still builds one at a time.
fn a_slots() -> &'static (std::sync::Mutex<usize>, std::sync::Condvar) {
    static S: std::sync::OnceLock<(std::sync::Mutex<usize>, std::sync::Condvar)> = std::sync::OnceLock::new();
    S.get_or_init(|| (std::sync::Mutex::new(0), std::sync::Condvar::new()))
}
impl Drop for AMatrix {
    fn drop(&mut self) {
        let (m, cv) = a_slots();
        *m.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        cv.notify_all();
    }
}
fn a_matrix(m: usize, row0: &[u8]) -> AMatrix {
    let bytes = m * k();
    let (lk, cv) = a_slots();
    let mut busy = lk.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        let room = crate::uv::mem_available_mb().is_none_or(|a| a / 2 >= (*busy as u64 + 1) * ((bytes as u64 >> 20) * 21 / 10));
        if *busy == 0 || (room && *busy < 8) {
            break;
        }
        busy = cv.wait_timeout(busy, std::time::Duration::from_millis(50)).unwrap_or_else(|e| e.into_inner()).0;
    }
    *busy += 1;
    drop(busy);
    let mut data = vec![64u8; bytes];
    data[..row0.len()].copy_from_slice(row0);
    AMatrix { data }
}

pub struct Job {
    pub header: IncompleteBlockHeader,
    #[allow(dead_code)]
    pub cert_version: u32,
    pub m: usize, // rows of A this job was built for (per device / SM group)
    pub deriv: SeedDerivation,
    pub job_key: [u8; 32],
    pub b_noise_seed: [u8; 32],
    b_col: Vec<u8>,  // B^T (column major), all 64
    /// CVs of the subtrees hanging off the left path of A's tree, bottom (row 0 = K/CHUNK chunks) to top.
    sibs: Vec<[u8; 32]>,
    nonce_base: u64,
    // B-side GPU inputs
    pub br_t: Vec<i8>,
    pub q1: Vec<i32>,
    pub q2: Vec<i32>,
}

impl Job {
    pub fn new(header_bytes: &[u8], cert_version: u32, m: usize) -> Result<Job> {
        let header = IncompleteBlockHeader::from_bytes(header_bytes)?;
        let deriv = CertificateVersion::try_from(cert_version)?.seed_derivation();
        config_sanity(header, deriv, m).context("public_params_sanity_check")?;
        let mut data = header.to_bytes().to_vec();
        data.extend_from_slice(&mining_config().to_bytes());
        let job_key = blake3_digest(&data, None);

        let kk = k();
        // A (m x k) is all 64 except row 0 (replaced per attempt): never kept in memory (1.7.3), see const_cv / a_matrix
        let a_len = m * kk;
        let b_col = vec![64u8; N * kk];
        ensure!(a_len % CHUNK == 0 && b_col.len() % CHUNK == 0 && kk % CHUNK == 0 && (kk / CHUNK).is_power_of_two());
        let hash_b = blake3_digest(&b_col, Some(job_key));
        let (_, bound_b) = deriv.bind_roots(&[0; 32], &hash_b, m as u32, N as u32);
        let b_noise_seed = blake3_digest(&[&job_key[..], &bound_b[..]].concat(), None);

        // Subtrees right of the left path (everything not containing the chunks of row 0).
        let mut n = a_len / CHUNK;
        let mut parts = vec![]; // (start chunk, chunk count), top to bottom
        while n > kk / CHUNK {
            let left = 1usize << (usize::BITS - 1 - (n - 1).leading_zeros()); // largest power of two < n
            parts.push((left, n - left)); // the left path always starts at chunk 0
            n = left;
        }
        let sibs = parts
            .iter()
            .rev()
            .map(|&(s, len)| const_cv(&job_key, s, len, CONST_BUF / CHUNK))
            .collect();

        let all_cols: Vec<usize> = (0..N).collect();
        let br_t: Vec<i8> = generate_uniform_random_matrix(&LABEL_B, &b_noise_seed, &all_cols, RANK).concat();
        let (q1, q2) = split_pairs(&generate_permutation_matrix(&LABEL_B, &b_noise_seed, kk, RANK));

        Ok(Job { header, cert_version, m, deriv, job_key, b_noise_seed, b_col, sibs, nonce_base: rand::random(), br_t, q1, q2 })
    }

    /// Nonce row (A row 0): K int8 values in [-64, 63] derived from (job_key, counter).
    pub fn nonce_row(&self, counter: u64) -> Vec<u8> {
        let mut h = blake3::Hasher::new();
        h.update(b"pearl-miner/nonce/v1");
        h.update(&self.job_key);
        h.update(&self.nonce_base.wrapping_add(counter).to_le_bytes());
        let mut out = vec![0u8; k()];
        h.finalize_xof().fill(&mut out);
        for b in out.iter_mut() {
            *b = ((*b & 0x7f) as i8 - 64) as u8;
        }
        out
    }

    /// Keyed BLAKE3 root of A (row 0 = `row0`, other rows 64) via cached subtree CVs.
    pub fn hash_a(&self, row0: &[u8]) -> [u8; 32] {
        let key = &self.job_key;
        let mode = Mode::KeyedHash(key);
        let mut cur = subtree_cv(key, row0, 0); // row 0 = the first K/CHUNK chunks (a perfect subtree)
        let last = self.sibs.len() - 1;
        for (i, s) in self.sibs.iter().enumerate() {
            if i == last {
                return *merge_subtrees_root(&cur, s, mode).as_bytes();
            }
            cur = merge_subtrees_non_root(&cur, s, mode);
        }
        unreachable!()
    }

    /// Reference: hash of the whole A with the given row 0.
    pub fn hash_a_full(&self, row0: &[u8]) -> [u8; 32] {
        let a = a_matrix(self.m, row0);
        blake3_digest(&a.data, Some(self.job_key))
    }

    pub fn a_noise_seed(&self, row0: &[u8]) -> [u8; 32] {
        let (bound_a, _) = self.deriv.bind_roots(&self.hash_a(row0), &[0; 32], self.m as u32, N as u32);
        blake3_digest(&[&self.b_noise_seed[..], &bound_a[..]].concat(), None)
    }

    /// Builds the plain proof for the jackpot tile at (t_rows, t_cols) of the attempt with `row0`.
    pub fn build_proof(&self, row0: &[u8], t_rows: usize, t_cols: usize) -> PlainProof {
        let cfg = mining_config();
        let a_rows: Vec<usize> = cfg.rows_pattern.to_list().iter().map(|&d| t_rows + d as usize).collect();
        let b_cols: Vec<usize> = cfg.cols_pattern.to_list().iter().map(|&d| t_cols + d as usize).collect();
        let a = a_matrix(self.m, row0);
        PlainProof {
            m: self.m,
            n: N,
            k: k(),
            noise_rank: RANK,
            a: matrix_proof(&a.data, &self.job_key, &a_rows, self.m),
            bt: matrix_proof(&self.b_col, &self.job_key, &b_cols, N),
            moe: None,
        }
    }

    /// Recomputes the jackpot hash from the plain proof (official code path, same as verify_plain_proof).
    /// Returns (jackpot hash, b_noise_seed, a_noise_seed as the verifier derives them).
    pub fn cpu_jackpot(&self, proof: &PlainProof) -> Result<([u8; 32], [u8; 32], [u8; 32])> {
        let (private, public) = proof.parse_proof(self.header, self.deriv)?;
        let compiled = CompiledPublicParams::from(&public);
        let noise = compute_noise(&compiled);
        let jackpot = compute_jackpot(&compiled, &private.s_a, &private.s_b, &noise);
        let (bs, as_) = compiled.commitment_hash;
        Ok((compute_jackpot_hash(&jackpot, as_), bs, as_))
    }

    /// Official verifier. Its own jackpot check is relaxed (easy nbits); the caller checks the pool target.
    pub fn verify(&self, proof: &PlainProof) -> Result<()> {
        verify_plain_proof(&self.header, proof, Some(0x207f_ffff), self.deriv)
    }
}

/// A-side permutation inputs for one attempt: generate_permutation_matrix(LABEL_A, seed, K, RANK) computed on the
/// calling thread (the official one fans 256 tiny hashes out to rayon). Checked against it in the self-test.
pub fn a_perm(a_noise_seed: &[u8; 32]) -> (Vec<i32>, Vec<i32>) {
    let kk = k();
    let (mut p1, mut p2) = (vec![0i32; kk], vec![0i32; kk]);
    for i in 0..kk / 8 {
        let h = get_random_hash(i, &LABEL_A, a_noise_seed, 1);
        for j in 0..8 {
            let u = u32::from_le_bytes(h[j * 4..j * 4 + 4].try_into().unwrap());
            let first = u & (RANK as u32 - 1);
            p1[i * 8 + j] = first as i32;
            p2[i * 8 + j] = (first ^ (1 + mul_hi_u32(RANK as u32 - 1, u))) as i32;
        }
    }
    (p1, p2)
}

/// Reference for a_perm (official, rayon).
pub fn a_perm_official(a_noise_seed: &[u8; 32]) -> (Vec<i32>, Vec<i32>) {
    split_pairs(&generate_permutation_matrix(&LABEL_A, a_noise_seed, k(), RANK))
}

fn split_pairs(p: &[[u32; 2]]) -> (Vec<i32>, Vec<i32>) {
    (p.iter().map(|x| x[0] as i32).collect(), p.iter().map(|x| x[1] as i32).collect())
}

/// Same as zk-pow's (private) build_matrix_proof, for already chunk-aligned data.
fn matrix_proof(data: &[u8], key: &[u8; 32], rows: &[usize], total_rows: usize) -> MatrixMerkleProof {
    assert_eq!(data.len() % CHUNK, 0);
    let tree = MerkleTree::new(data, *key);
    let leaves = MerkleTree::compute_leaf_indices_from_rows(rows, (total_rows, k()));
    MatrixMerkleProof { proof: tree.get_multileaf_proof(&leaves), row_indices: rows.to_vec() }
}
