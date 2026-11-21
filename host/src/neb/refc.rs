// FFGN — see LICENSE
//! The NEB C reference of an attempt (`host/nebref/*.c`, copied from `pow/ref` of NEB v1.0.0 = v1.0.1, unchanged, Apache-2.0, see NOTICE),
//! compiled into the miner by build.rs.  Every nonce a GPU reports is evaluated again here before it is sent.
//! Also: the dataset file (`neb-datasets.bin`: NMDS set of NMD1 / NMD2 datasets, BLAKE3 pinned) and models from packed weights.
use std::ffi::{c_int, c_void};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Arch {
    /// 784-128-10 (spec/shares.md; testnet before block 8800 only)
    V1,
    /// Kepler transit signals (spec/nebhash-v2.md 3.1)
    Kep1,
    /// retinal scans 64 x 64 (spec/nebhash-v2.md 3.2)
    Ret2,
}

pub const N1: usize = 128 * 784;
pub const N2: usize = 10 * 128;

impl Arch {
    pub fn id(self) -> i32 {
        match self {
            Arch::V1 => 0,
            Arch::Kep1 => 1,
            Arch::Ret2 => 2,
        }
    }
    pub fn nin(self) -> usize {
        match self {
            Arch::V1 | Arch::Kep1 => 784,
            Arch::Ret2 => 4096,
        }
    }
    pub fn nout(self) -> usize {
        match self {
            Arch::V1 => 10,
            _ => 2,
        }
    }
    pub fn weights(self) -> usize {
        match self {
            Arch::V1 => N1 + N2,
            Arch::Kep1 => 26_000,
            Arch::Ret2 => 22_280,
        }
    }
    pub fn packed_bytes(self) -> usize {
        self.weights() / 2
    }
    pub fn name(self) -> &'static str {
        match self {
            Arch::V1 => "v1",
            Arch::Kep1 => "KEP1",
            Arch::Ret2 => "RET2",
        }
    }
    pub fn from_packed_len(n: usize) -> Option<Arch> {
        [Arch::V1, Arch::Kep1, Arch::Ret2].into_iter().find(|a| a.packed_bytes() == n)
    }
}

const MAX_W2: usize = 26_000;

#[repr(C)]
struct V1Model {
    w1: [i8; N1],
    w2: [i8; N2],
    r1: [i32; N1],
    r2: [i32; N2],
}

#[repr(C)]
struct V2Model {
    arch: i32,
    w: [i8; MAX_W2],
    r: [i32; MAX_W2],
}

#[repr(C)]
struct CDataset {
    n: u32,
    get: extern "C" fn(*mut c_void, u32, *mut u8) -> c_int,
    ctx: *mut c_void,
}

unsafe extern "C" {
    fn neb_share_eval(m: *const V1Model, ds: *const CDataset, key: *const u8, g1: *mut i32, g2: *mut i32, digest: *mut u8, ticket: *mut u8, idx: *mut u32, nclip: *mut u32);
    fn neb2_share_eval(m: *const V2Model, ds: *const CDataset, key: *const u8, g: *mut i32, digest: *mut u8, ticket: *mut u8, idx: *mut u32, nclip: *mut u32) -> c_int;
}

fn zeroed<T>() -> Box<T> {
    // SAFETY: T is one of the integer-only C mirrors above: all-zero bytes are a valid value
    unsafe {
        let layout = std::alloc::Layout::new::<T>();
        let p = std::alloc::alloc_zeroed(layout) as *mut T;
        assert!(!p.is_null(), "out of memory");
        Box::from_raw(p)
    }
}

enum Inner {
    V1(Box<V1Model>),
    V2(Box<V2Model>),
}

/// The model of a job: packed weights (as the node sends them), unpacked weights, the C state (zero residuals: the ticket
/// depends on the weights only).
pub struct Model {
    pub arch: Arch,
    pub hash: [u8; 32],
    pub packed: Vec<u8>,
    /// the weights in model order, -8..7 (what a GPU library is given)
    pub weights: Vec<i8>,
    inner: Inner,
}

// SAFETY: the C state is plain data, read-only after construction
unsafe impl Send for Model {}
unsafe impl Sync for Model {}

/// weights_hash of spec/shares.md section 7 / spec/nebhash-v2.md section 6: BLAKE3("neb-model-weights" || packed weights)
pub fn weights_hash(packed: &[u8]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(b"neb-model-weights");
    h.update(packed);
    *h.finalize().as_bytes()
}

impl Model {
    /// A model from a Model frame / `mining.set_model`: the architecture from the size, the hash checked.
    pub fn from_packed(packed: &[u8], want_hash: &[u8; 32]) -> Result<Model, String> {
        let arch = Arch::from_packed_len(packed.len()).ok_or_else(|| format!("model of {} bytes: not a known network", packed.len()))?;
        let hash = weights_hash(packed);
        if &hash != want_hash {
            return Err("the weights do not match their hash".into());
        }
        let signed = |n: u8| ((n << 4) as i8) >> 4;
        let weights: Vec<i8> = packed.iter().flat_map(|b| [signed(b & 15), signed(b >> 4)]).collect();
        let inner = match arch {
            Arch::V1 => {
                let mut m = zeroed::<V1Model>();
                m.w1.copy_from_slice(&weights[..N1]);
                m.w2.copy_from_slice(&weights[N1..]);
                Inner::V1(m)
            }
            _ => {
                let mut m = zeroed::<V2Model>();
                m.arch = arch.id();
                m.w[..weights.len()].copy_from_slice(&weights);
                Inner::V2(m)
            }
        };
        Ok(Model { arch, hash, packed: packed.to_vec(), weights, inner })
    }
}

/// One dataset of the file: `n` records of `nin` values (0..7) and a label, inside the shared file bytes.
pub struct Dataset {
    pub arch: Arch,
    pub n: u32,
    bytes: Arc<Vec<u8>>,
    off: usize,
}

impl Dataset {
    pub fn record(&self, i: u32) -> &[u8] {
        let r = self.arch.nin() + 1;
        &self.bytes[self.off + i as usize * r..][..r]
    }
    /// all values (n x nin) then all labels: the layout a GPU library is given
    pub fn flatten(&self) -> (Vec<u8>, Vec<u8>) {
        let nin = self.arch.nin();
        let mut v = Vec::with_capacity(self.n as usize * nin);
        let mut l = Vec::with_capacity(self.n as usize);
        for i in 0..self.n {
            let r = self.record(i);
            v.extend_from_slice(&r[..nin]);
            l.push(r[nin]);
        }
        (v, l)
    }
}

/// The dataset files of the networks (docs/mainnet.md, docs/testnet.md of NEB 1.0.1).  The main network of NEB 1.0.1 has
/// two KEP1 datasets (slot 0 Kepler, slot 1 TESS); the file of NEB 1.0.0 (KEP1 + RET2) is refused with a hint.
pub const MAINNET_SIZE: u64 = 16_087_037;
pub const MAINNET_BLAKE3: &str = "790b57fbd830f3dcbe7de9d1146aa89a0a20799118e30dd48a343a5c98f19e72";
pub const MAINNET_URL: &str = "https://github.com/SilasEigen/neb/releases/download/v1.0.1/neb-datasets.bin";
/// The main-network file of NEB 1.0.0, replaced by 1.0.1 before the start (a 1.0.1 node refuses it).
pub const OLD_MAINNET_100_BLAKE3: &str = "fbabd0bdde0b43aef946e7031ff6e037c8a2a5d0ae2616b49b2b49b11af8010e";
pub const TESTNET_SIZE: u64 = 259_897_080;
pub const TESTNET_BLAKE3: &str = "296a174edbf67339fa0af341f063154292bba224b3d2cacf5b18b31c70da80eb";

/// The dataset of this file for a job's slot index and network (1.0.2): the slot index, if it holds a dataset of the job's
/// network.  The network of a slot is that of its dataset (NMD2 architecture id), never of the slot number: the main network
/// of NEB 1.0.1 has KEP1 in both slots (0 Kepler, 1 TESS); the test network's file has 4 datasets (v1, v1, KEP1, RET2) and
/// its v2 jobs name index 2 / 3.  No cross-file mapping any more (1.0.1 mapped test jobs onto the 1.0.0 main file): slot 1 of
/// the main network is TESS data, which no other file holds, so a job is mined only with the file of its own network.
/// Err = this file cannot mine the job (said loudly, with the file to use).
pub fn resolve_dataset(sets: &[Arc<Dataset>], slot: u8, arch: Arch) -> Result<usize, String> {
    match sets.get(slot as usize) {
        Some(d) if d.arch == arch => Ok(slot as usize),
        Some(d) => Err(format!(
            "the job is for dataset {slot} ({} network) but dataset {slot} of this file is {}: the dataset file of another network (main network jobs name datasets 0 / 1: use neb-datasets.bin of NEB 1.0.1, {MAINNET_URL}; test network jobs name 2 / 3: use the testnet file of NEB 0.3.1)",
            arch.name(),
            d.arch.name()
        )),
        None => Err(format!(
            "the job is for dataset {slot} ({} network) and this file has {} dataset(s): the dataset file of another network (test network jobs name datasets 2 / 3: use the testnet file of NEB 0.3.1; main network: neb-datasets.bin of NEB 1.0.1)",
            arch.name(),
            sets.len()
        )),
    }
}

/// 1.0.4: the name of a job's dataset for the stats line, the API and HiveOS: main network slot 0 "Kepler", slot 1 "TESS"
/// (the rate differs by slot: RTX 3070 @1650 ~16.9 vs ~18.6 MA/s, a slot change is not a regression); else the architecture.
pub fn slot_name(network: &str, slot: u8, arch: Arch) -> &'static str {
    match (network, slot) {
        ("mainnet", 0) => "Kepler",
        ("mainnet", 1) => "TESS",
        _ => arch.name(),
    }
}

pub struct DatasetFile {
    pub hash: [u8; 32],
    /// the whole file (shared with the datasets; handed to the GPU engine as is)
    pub bytes: Arc<Vec<u8>>,
    pub network: &'static str,
    pub sets: Vec<Arc<Dataset>>,
}

/// Reads `neb-datasets.bin`; the BLAKE3 hash must be the mainnet's or the testnet's (or `allow_other`, development only).
pub fn load_datasets(path: &std::path::Path, allow_other: bool) -> Result<DatasetFile, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read the dataset file {}: {e}", path.display()))?;
    let hash = *blake3::hash(&bytes).as_bytes();
    let hx = crate::neb::proto::hex(&hash);
    let network = if hx == MAINNET_BLAKE3 {
        "mainnet"
    } else if hx == TESTNET_BLAKE3 {
        "testnet"
    } else if hx == OLD_MAINNET_100_BLAKE3 && !allow_other {
        return Err(format!(
            "{} is the dataset file of NEB 1.0.0 (222,320,684 bytes, BLAKE3 {}…), replaced by NEB 1.0.1 before the start of the main network: a NEB 1.0.1 node and pool refuse it.  Download neb-datasets.bin of NEB 1.0.1 ({MAINNET_SIZE} bytes, BLAKE3 {}…): {MAINNET_URL} (or take the one in the FFGN 1.0.2 package)",
            path.display(),
            &OLD_MAINNET_100_BLAKE3[..16],
            &MAINNET_BLAKE3[..16]
        ));
    } else if allow_other {
        "other"
    } else {
        return Err(format!(
            "{} ({} bytes) has BLAKE3 {hx}: not the dataset file of the NEB main network ({MAINNET_SIZE} bytes, {}…) nor of the test network; download neb-datasets.bin of NEB 1.0.1: {MAINNET_URL}",
            path.display(),
            bytes.len(),
            &MAINNET_BLAKE3[..16]
        ));
    };
    let bytes = Arc::new(bytes);
    let sets = parse_sets(bytes.clone())?;
    Ok(DatasetFile { hash, bytes, network, sets })
}

fn header(b: &[u8]) -> Result<(Arch, usize), String> {
    match b.get(..4) {
        Some(m) if m == b"NMD1" => Ok((Arch::V1, 8)),
        Some(m) if m == b"NMD2" => {
            let id = u32::from_le_bytes(b.get(4..8).ok_or("NMD2 header cut short")?.try_into().unwrap());
            Ok((match id {
                1 => Arch::Kep1,
                2 => Arch::Ret2,
                x => return Err(format!("unknown architecture id {x}")),
            }, 12))
        }
        _ => Err("not a dataset (bad magic)".into()),
    }
}

pub fn parse_sets(bytes: Arc<Vec<u8>>) -> Result<Vec<Arc<Dataset>>, String> {
    let b = &bytes[..];
    let (k, mut at) = if b.len() >= 8 && &b[..4] == b"NMDS" { (u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize, 8) } else { (1, 0) };
    if k == 0 || k > 16 {
        return Err(format!("a dataset set holds 1 to 16 datasets, not {k}"));
    }
    let mut out = Vec::new();
    for i in 0..k {
        let (arch, head) = header(&b[at..]).map_err(|e| format!("dataset {i}: {e}"))?;
        let n = u32::from_le_bytes(b.get(at + head - 4..at + head).ok_or_else(|| format!("dataset {i} cut short"))?.try_into().unwrap());
        let rec = arch.nin() + 1;
        let end = (n as usize).checked_mul(rec).and_then(|s| s.checked_add(at + head)).filter(|e| *e <= b.len()).ok_or_else(|| format!("dataset {i} cut short"))?;
        if n == 0 {
            return Err(format!("dataset {i} is empty"));
        }
        if b[at + head..end].chunks(rec).any(|r| r[..arch.nin()].iter().any(|&p| p > 7) || r[arch.nin()] as usize >= arch.nout()) {
            return Err(format!("dataset {i}: a value above 7 or a bad label"));
        }
        out.push(Arc::new(Dataset { arch, n, bytes: bytes.clone(), off: at + head }));
        at = end;
    }
    if at != b.len() {
        return Err("bytes after the last dataset".into());
    }
    Ok(out)
}

extern "C" fn sample(ctx: *mut c_void, index: u32, x: *mut u8) -> c_int {
    // SAFETY: ctx is the &Dataset passed by `ticket` below, alive for the C call; x holds nin bytes (the C code's buffer for
    // the model's architecture, which `ticket` checked equals the dataset's)
    let ds = unsafe { &*(ctx as *const Dataset) };
    let nin = ds.arch.nin();
    if index >= ds.n {
        return 0; // never: the C code draws indices below n
    }
    let r = ds.record(index);
    unsafe { std::ptr::copy_nonoverlapping(r.as_ptr(), x, nin) };
    r[nin] as c_int
}

/// Per-thread scratch of the reference.
pub struct Scratch {
    g: Vec<i32>,
    g2: Vec<i32>,
}

impl Default for Scratch {
    fn default() -> Self {
        Scratch { g: vec![0; N1.max(MAX_W2)], g2: vec![0; N2] }
    }
}

/// The ticket of `key` = BLAKE3(header) on `model` and `ds` (the C reference).
pub fn ticket(sc: &mut Scratch, model: &Model, ds: &Dataset, key: &[u8; 32]) -> Result<[u8; 32], String> {
    if model.arch != ds.arch {
        return Err(format!("the model is {} but the dataset is {}", model.arch.name(), ds.arch.name()));
    }
    let cds = CDataset { n: ds.n, get: sample, ctx: ds as *const Dataset as *mut c_void };
    let (mut digest, mut t) = ([0u8; 32], [0u8; 32]);
    match &model.inner {
        // SAFETY: the buffers have the sizes the C functions write (N1/N2, the arch's weight count); the dataset outlives the call
        Inner::V1(m) => unsafe {
            neb_share_eval(&**m, &cds, key.as_ptr(), sc.g.as_mut_ptr(), sc.g2.as_mut_ptr(), digest.as_mut_ptr(), t.as_mut_ptr(), std::ptr::null_mut(), std::ptr::null_mut())
        },
        Inner::V2(m) => {
            let rc = unsafe { neb2_share_eval(&**m, &cds, key.as_ptr(), sc.g.as_mut_ptr(), digest.as_mut_ptr(), t.as_mut_ptr(), std::ptr::null_mut(), std::ptr::null_mut()) };
            if rc != 0 {
                return Err("the reference evaluation failed (out of memory)".into());
            }
        }
    }
    Ok(t)
}

/// Ticket of `nonce` on the 80-byte header.
pub fn ticket_of(sc: &mut Scratch, model: &Model, ds: &Dataset, header: &[u8; 80], nonce: u32) -> Result<[u8; 32], String> {
    let mut h = *header;
    h[76..80].copy_from_slice(&nonce.to_le_bytes());
    ticket(sc, model, ds, blake3::hash(&h).as_bytes())
}

unsafe extern "C" {
    fn neb2_model_init(arch: c_int, seed: *const u8, m: *mut V2Model) -> c_int;
    fn chacha8_stream(key: *const u8, out: *mut u8, n: usize);
}

fn chacha8(key: &[u8; 32], out: &mut [u8]) {
    // SAFETY: reads 32 bytes of key, writes out.len() bytes
    unsafe { chacha8_stream(key.as_ptr(), out.as_mut_ptr(), out.len()) };
}

/// Known-answer check of the C reference as built into this binary, on the vectors of NEB (`vectors/nebhash2.txt`, step 0
/// of each network: the genesis-seed model and the synthetic dataset of spec/nebhash-v2.md section 8).
pub fn self_check() -> Result<(), String> {
    const KAT: [(Arch, [&str; 4]); 2] = [
        (Arch::Kep1, [
            "62e9520e82d11479d309864729776e4b57e97d06d1e6a82224086116e276e134",
            "8a31fab87a13b665e98781580c937a577dd0fa704a86ef3b83f02d391f45e234",
            "e06a83f946739688fb538b412c55330242f825fdb0096ed99c697f9538d3a10f",
            "97a64d820faffd83e4c5cdf378a1e82a18949f05ff077fc3267e1660ada591eb",
        ]),
        (Arch::Ret2, [
            "7bb54bea4cfb593e3727700daf3c1610bc91e79d78536e65c3f8f6025aa4a56b",
            "eb8a0d21807086b5df77fd99ee57f049f2db9b7c0fa0aca1b141dc8637b0dbf3",
            "9d760933bbdf8dfd6cfb3a3257eb9ec0b66536629a70f8b2ba6e0995f12ecc0d",
            "8d5f592b57c69e56cc15e96cd0d4c04fd40a170b7ceb23b199aa1ffe3422b8cd",
        ]),
    ];
    let mut sc = Scratch::default();
    for (arch, want) in KAT {
        let (ds, model) = synthetic(arch)?;
        for (j, w) in want.iter().enumerate() {
            let mut input = [0u8; 24];
            input[..15].copy_from_slice(b"neb-test-share2");
            input[15] = arch.id() as u8;
            input[20..].copy_from_slice(&(j as u32).to_le_bytes());
            let t = ticket(&mut sc, &model, &ds, blake3::hash(&input).as_bytes())?;
            if crate::neb::proto::hex(&t) != *w {
                return Err(format!("{} vector share {j}: ticket {} differs from the NEB vector", arch.name(), crate::neb::proto::hex(&t)));
            }
        }
    }
    Ok(())
}

/// The synthetic dataset (2048 samples) and the seed-0 model of a v2 network (test vectors; also the GPU self-test).
pub fn synthetic(arch: Arch) -> Result<(Dataset, Model), String> {
    let nin = arch.nin();
    let id = arch.id() as u8;
    let protos: Vec<Vec<u8>> = (0u8..2)
        .map(|c| {
            let mut input = [0u8; 16];
            input[..14].copy_from_slice(b"neb-syn2-proto");
            input[14] = id;
            input[15] = c;
            let mut p = vec![0u8; nin];
            chacha8(blake3::hash(&input).as_bytes(), &mut p);
            p.iter_mut().for_each(|v| *v &= 7);
            p
        })
        .collect();
    const N: u32 = 2048;
    let mut bytes = Vec::with_capacity(12 + N as usize * (nin + 1));
    bytes.extend_from_slice(b"NMD2");
    bytes.extend_from_slice(&(id as u32).to_le_bytes());
    bytes.extend_from_slice(&N.to_le_bytes());
    let mut noise = vec![0u8; nin];
    for i in 0..N {
        let mut input = [0u8; 20];
        input[..15].copy_from_slice(b"neb-syn2-sample");
        input[15] = id;
        input[16..].copy_from_slice(&i.to_le_bytes());
        chacha8(blake3::hash(&input).as_bytes(), &mut noise);
        let p = &protos[(i % 2) as usize];
        for k in 0..nin {
            bytes.push((p[k] as i32 + (noise[k] & 3) as i32 - 1).clamp(0, 7) as u8);
        }
        bytes.push((i % 2) as u8);
    }
    let ds = parse_sets(Arc::new(bytes))?.pop().unwrap();
    let mut m = zeroed::<V2Model>();
    // SAFETY: known architecture id, 32-byte seed
    if unsafe { neb2_model_init(arch.id(), [0u8; 32].as_ptr(), &mut *m) } != 0 {
        return Err("neb2_model_init failed".into());
    }
    let w = &m.w[..arch.weights()];
    let packed: Vec<u8> = w.chunks(2).map(|p| ((p[0] as u8) & 15) | (((p[1] as u8) & 15) << 4)).collect();
    let model = Model::from_packed(&packed, &weights_hash(&packed))?;
    let ds = Arc::try_unwrap(ds).map_err(|_| "dataset shared")?;
    Ok((ds, model))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_file(archs: &[u32]) -> Vec<Arc<Dataset>> {
        let mut b = b"NMDS".to_vec();
        b.extend_from_slice(&(archs.len() as u32).to_le_bytes());
        for &a in archs {
            if a == 0 {
                b.extend_from_slice(b"NMD1");
                b.extend_from_slice(&1u32.to_le_bytes());
                b.extend_from_slice(&[0u8; 785]);
            } else {
                b.extend_from_slice(b"NMD2");
                b.extend_from_slice(&a.to_le_bytes());
                b.extend_from_slice(&1u32.to_le_bytes());
                b.extend_from_slice(&vec![0u8; if a == 1 { 785 } else { 4097 }]);
            }
        }
        parse_sets(Arc::new(b)).unwrap()
    }

    #[test]
    fn jobs_of_both_networks_find_their_dataset() {
        let main = set_file(&[1, 1]); // main network of NEB 1.0.1: KEP1 (Kepler), KEP1 (TESS)
        let test = set_file(&[0, 0, 1, 2]); // test network: v1, v1, KEP1, RET2
        let old = set_file(&[1, 2]); // main network of NEB 1.0.0: KEP1, RET2
        // main-network jobs: slot 0 and slot 1 are both KEP1, each on its own dataset
        assert_eq!(resolve_dataset(&main, 0, Arch::Kep1), Ok(0));
        assert_eq!(resolve_dataset(&main, 1, Arch::Kep1), Ok(1));
        assert!(resolve_dataset(&main, 1, Arch::Ret2).is_err()); // no RET2 on the main network any more
        // test-network jobs (slot 2 / 3) only with the test-network file
        assert_eq!(resolve_dataset(&test, 2, Arch::Kep1), Ok(2));
        assert_eq!(resolve_dataset(&test, 3, Arch::Ret2), Ok(3));
        assert!(resolve_dataset(&main, 2, Arch::Kep1).is_err());
        assert!(resolve_dataset(&main, 3, Arch::Ret2).is_err());
        // a main-network job with the test-network file is never mined on other data (slot 1 = TESS exists nowhere else)
        assert!(resolve_dataset(&test, 0, Arch::Kep1).is_err());
        assert!(resolve_dataset(&test, 1, Arch::Kep1).is_err());
        // the old 1.0.0 layout: a slot-1 KEP1 job is not mined on its RET2 dataset
        assert!(resolve_dataset(&old, 1, Arch::Kep1).is_err());
        // v1 jobs: only where the file has v1 data
        assert!(resolve_dataset(&main, 0, Arch::V1).is_err());
        assert!(resolve_dataset(&test, 1, Arch::V1).is_ok());
    }

    #[test]
    fn c_reference_matches_the_neb_vectors() {
        super::self_check().unwrap();
    }
}
