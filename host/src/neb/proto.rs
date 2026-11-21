// FFGN — see LICENSE
//! NEB wire formats used by the miner: the work-service frames of a NEB node (spec/node.md section 7.1), the job and its
//! 80-byte header (coinbase leaf, Merkle branch, body root), targets (256-bit little-endian), hex helpers.
//!
//! Written from the NEB specification; the frame layout follows `node/crates/neb-work/src/frames.rs` of NEB v1.0.0
//! (Apache-2.0, see NOTICE).  Every decoder is canonical and bounds its allocations (the peer may be remote through a gate).

pub const MSG_HELLO: u8 = 1;
pub const MSG_JOB: u8 = 2;
pub const MSG_SUBMIT: u8 = 4;
pub const MSG_RESULT: u8 = 5;
pub const MSG_SHARE: u8 = 6;
pub const MSG_MODEL: u8 = 7;
/// largest `length` field (type byte included): 2 MiB
pub const MAX_FRAME: usize = 2 * 1024 * 1024;

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

pub fn unhex32(s: &str) -> Option<[u8; 32]> {
    unhex(s)?.try_into().ok()
}

/// `ticket <= target`, both 256-bit little-endian.
pub fn admits(target: &[u8; 32], ticket: &[u8; 32]) -> bool {
    for i in (0..32).rev() {
        if ticket[i] != target[i] {
            return ticket[i] < target[i];
        }
    }
    true
}

/// 256-bit little-endian value as f64 (for rates and estimates only).
pub fn to_f64(v: &[u8; 32]) -> f64 {
    v.iter().rev().fold(0.0, |a, &b| a * 256.0 + b as f64)
}

/// Expected attempts per hit of `target`: 2^256 / (target + 1).
pub fn expected(target: &[u8; 32]) -> f64 {
    2f64.powi(256) / (to_f64(target) + 1.0)
}

/// `value - 1`; None for 0.
pub fn minus_one(v: &[u8; 32]) -> Option<[u8; 32]> {
    let mut o = *v;
    for b in o.iter_mut() {
        if *b == 0 {
            *b = 0xff;
        } else {
            *b -= 1;
            return Some(o);
        }
    }
    None
}

/// The tagged hash of spec/node.md section 3: BLAKE3(tag || parts...).
pub fn h(tag: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut x = blake3::Hasher::new();
    x.update(tag);
    for p in parts {
        x.update(p);
    }
    *x.finalize().as_bytes()
}

pub fn frame(t: u8, payload: &[u8]) -> Vec<u8> {
    let mut o = Vec::with_capacity(5 + payload.len());
    o.extend_from_slice(&((payload.len() + 1) as u32).to_le_bytes());
    o.push(t);
    o.extend_from_slice(payload);
    o
}

/// The first frame of `buf`: Ok(None) = incomplete, Ok(Some((type, payload range, consumed))).
pub fn parse_frame(buf: &[u8]) -> Result<Option<(u8, std::ops::Range<usize>, usize)>, String> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(format!("frame length {len} out of range"));
    }
    if buf.len() < 4 + len {
        return Ok(None);
    }
    let t = buf[4];
    if !(1..=7).contains(&t) {
        return Err(format!("unknown frame type {t}"));
    }
    Ok(Some((t, 5..4 + len, 4 + len)))
}

pub fn write_varint(o: &mut Vec<u8>, v: u64) {
    // spec/node.md section 3: Bitcoin CompactSize
    if v < 0xfd {
        o.push(v as u8);
    } else if v <= 0xffff {
        o.push(0xfd);
        o.extend_from_slice(&(v as u16).to_le_bytes());
    } else if v <= 0xffff_ffff {
        o.push(0xfe);
        o.extend_from_slice(&(v as u32).to_le_bytes());
    } else {
        o.push(0xff);
        o.extend_from_slice(&v.to_le_bytes());
    }
}

pub struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Reader { b, at: 0 }
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.b.len()).ok_or("payload cut short")?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }
    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.bytes(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }
    pub fn b32(&mut self) -> Result<[u8; 32], String> {
        Ok(self.bytes(32)?.try_into().unwrap())
    }
    /// canonical CompactSize
    pub fn varint(&mut self) -> Result<u64, String> {
        let p = self.u8()?;
        let (v, min) = match p {
            0xfd => (self.u16()? as u64, 0xfd),
            0xfe => (self.u32()? as u64, 0x1_0000),
            0xff => (self.u64()?, 0x1_0000_0000),
            x => return Ok(x as u64),
        };
        if v < min {
            return Err("non-canonical varint".into());
        }
        Ok(v)
    }
    pub fn var_bytes(&mut self) -> Result<Vec<u8>, String> {
        let n = self.varint()?;
        if n > MAX_FRAME as u64 {
            return Err("byte string too long".into());
        }
        Ok(self.bytes(n as usize)?.to_vec())
    }
    pub fn done(&self) -> Result<(), String> {
        if self.at == self.b.len() { Ok(()) } else { Err("trailing bytes".into()) }
    }
}

/// The job of spec/node.md section 7.1 (also the content of a neb-pool/1 `mining.notify`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub id: u64,
    pub clean: bool,
    pub version: u32,
    pub parent: [u8; 32],
    pub time_start: u32,
    pub time_window: u32,
    pub bits: u32,
    pub target: [u8; 32],
    pub share_target: [u8; 32],
    pub coinbase_prefix: Vec<u8>,
    pub coinbase_suffix: Vec<u8>,
    pub extranonce_size: u8,
    pub branch: Vec<[u8; 32]>,
    pub tx_count: u32,
    pub shares_root: [u8; 32],
    pub height: u32,
    pub expiry: u32,
}

impl Job {
    pub fn body_root(&self, extranonce: &[u8]) -> [u8; 32] {
        let mut cb = Vec::with_capacity(self.coinbase_prefix.len() + extranonce.len() + self.coinbase_suffix.len());
        cb.extend_from_slice(&self.coinbase_prefix);
        cb.extend_from_slice(extranonce);
        cb.extend_from_slice(&self.coinbase_suffix);
        let mut acc = h(b"leaf", &[blake3::hash(&cb).as_bytes()]);
        for s in &self.branch {
            acc = h(b"node", &[&acc, s]);
        }
        h(b"body", &[&self.tx_count.to_le_bytes(), &acc, &self.shares_root])
    }

    /// 80-byte header: version || parent || body_root || time || bits || nonce
    pub fn header(&self, extranonce: &[u8], time: u32, nonce: u32) -> [u8; 80] {
        let mut o = [0u8; 80];
        o[0..4].copy_from_slice(&self.version.to_le_bytes());
        o[4..36].copy_from_slice(&self.parent);
        o[36..68].copy_from_slice(&self.body_root(extranonce));
        o[68..72].copy_from_slice(&time.to_le_bytes());
        o[72..76].copy_from_slice(&self.bits.to_le_bytes());
        o[76..80].copy_from_slice(&nonce.to_le_bytes());
        o
    }

    pub fn encode_payload(&self) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(&self.id.to_le_bytes());
        o.push(self.clean as u8);
        o.extend_from_slice(&self.version.to_le_bytes());
        o.extend_from_slice(&self.parent);
        o.extend_from_slice(&self.time_start.to_le_bytes());
        o.extend_from_slice(&self.time_window.to_le_bytes());
        o.extend_from_slice(&self.bits.to_le_bytes());
        o.extend_from_slice(&self.target);
        o.extend_from_slice(&self.share_target);
        write_varint(&mut o, self.coinbase_prefix.len() as u64);
        o.extend_from_slice(&self.coinbase_prefix);
        write_varint(&mut o, self.coinbase_suffix.len() as u64);
        o.extend_from_slice(&self.coinbase_suffix);
        o.push(self.extranonce_size);
        write_varint(&mut o, self.branch.len() as u64);
        for b in &self.branch {
            o.extend_from_slice(b);
        }
        o.extend_from_slice(&self.tx_count.to_le_bytes());
        o.extend_from_slice(&self.shares_root);
        o.extend_from_slice(&self.height.to_le_bytes());
        o.extend_from_slice(&self.expiry.to_le_bytes());
        o
    }

    pub fn decode(p: &[u8]) -> Result<Job, String> {
        let mut r = Reader::new(p);
        let id = r.u64()?;
        let clean = match r.u8()? {
            0 => false,
            1 => true,
            _ => return Err("clean is not 0 or 1".into()),
        };
        let version = r.u32()?;
        let parent = r.b32()?;
        let time_start = r.u32()?;
        let time_window = r.u32()?;
        let bits = r.u32()?;
        let target = r.b32()?;
        let share_target = r.b32()?;
        let coinbase_prefix = r.var_bytes()?;
        let coinbase_suffix = r.var_bytes()?;
        let extranonce_size = r.u8()?;
        if extranonce_size == 0 || extranonce_size > 32 {
            return Err("extranonce size out of range".into());
        }
        let n = r.varint()?;
        if n > 4096 {
            return Err("branch too long".into());
        }
        let mut branch = Vec::with_capacity(n as usize);
        for _ in 0..n {
            branch.push(r.b32()?);
        }
        let tx_count = r.u32()?;
        if tx_count == 0 {
            return Err("zero transactions".into());
        }
        let shares_root = r.b32()?;
        let height = r.u32()?;
        let expiry = r.u32()?;
        r.done()?;
        Ok(Job { id, clean, version, parent, time_start, time_window, bits, target, share_target, coinbase_prefix, coinbase_suffix, extranonce_size, branch, tx_count, shares_root, height, expiry })
    }
}

pub fn hello(name: &str) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&1u16.to_le_bytes());
    write_varint(&mut p, name.len() as u64);
    p.extend_from_slice(name.as_bytes());
    frame(MSG_HELLO, &p)
}

pub fn share(job_id: u64, nonce: u32) -> Vec<u8> {
    let mut p = Vec::with_capacity(12);
    p.extend_from_slice(&job_id.to_le_bytes());
    p.extend_from_slice(&nonce.to_le_bytes());
    frame(MSG_SHARE, &p)
}

pub fn submit(job_id: u64, extranonce: &[u8], time: u32, nonce: u32) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&job_id.to_le_bytes());
    write_varint(&mut p, extranonce.len() as u64);
    p.extend_from_slice(extranonce);
    p.extend_from_slice(&time.to_le_bytes());
    p.extend_from_slice(&nonce.to_le_bytes());
    frame(MSG_SUBMIT, &p)
}

pub fn decode_share(p: &[u8]) -> Result<(u64, u32), String> {
    let mut r = Reader::new(p);
    let v = (r.u64()?, r.u32()?);
    r.done()?;
    Ok(v)
}

pub fn decode_submit(p: &[u8]) -> Result<(u64, Vec<u8>, u32, u32), String> {
    let mut r = Reader::new(p);
    let v = (r.u64()?, r.var_bytes()?, r.u32()?, r.u32()?);
    r.done()?;
    Ok(v)
}

pub fn decode_hello(p: &[u8]) -> Result<(u16, String), String> {
    let mut r = Reader::new(p);
    let v = r.u16()?;
    let n = String::from_utf8(r.var_bytes()?).map_err(|_| "name is not UTF-8")?;
    r.done()?;
    Ok((v, n))
}

/// Result frame: (accepted, code); code 0 accepted, 1 bad submission, 2 bad header, 3 bad state.
pub fn decode_result(p: &[u8]) -> Result<(bool, u8), String> {
    if p.len() != 2 {
        return Err("result is two bytes".into());
    }
    Ok((p[0] == 1, p[1]))
}

pub fn result_frame(accepted: bool, code: u8) -> Vec<u8> {
    frame(MSG_RESULT, &[accepted as u8, code])
}

pub fn result_text(code: u8) -> &'static str {
    match code {
        0 => "accepted",
        1 => "bad submission (unknown job, extranonce length, time window or coinbase)",
        2 => "bad header (header check, proof of work or a share of the block)",
        3 => "bad state (the block was not connected)",
        _ => "unknown code",
    }
}

/// Model frame payload: slot || weights_hash || packed weights; returns (slot, hash, packed).
pub fn split_model(p: &[u8]) -> Result<(u8, [u8; 32], &[u8]), String> {
    if p.len() < 34 {
        return Err("model frame too short".into());
    }
    Ok((p[0], p[1..33].try_into().unwrap(), &p[33..]))
}

pub fn model_frame(slot: u8, hash: &[u8; 32], packed: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(33 + packed.len());
    p.push(slot);
    p.extend_from_slice(hash);
    p.extend_from_slice(packed);
    frame(MSG_MODEL, &p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> Job {
        Job {
            id: 7,
            clean: true,
            version: 1,
            parent: [3; 32],
            time_start: 1_794_150_000,
            time_window: 3600,
            bits: 0xd580_0000,
            target: [0xff; 32],
            share_target: [0xff; 32],
            coinbase_prefix: vec![1, 2, 3],
            coinbase_suffix: vec![9; 300],
            extranonce_size: 12,
            branch: vec![[5; 32], [6; 32]],
            tx_count: 3,
            shares_root: [8; 32],
            height: 2454,
            expiry: 1_794_150_300,
        }
    }

    #[test]
    fn job_round_trip_and_header_layout() {
        let j = job();
        let p = j.encode_payload();
        assert_eq!(Job::decode(&p).unwrap(), j);
        let mut bad = p.clone();
        bad.push(0);
        assert!(Job::decode(&bad).is_err());
        let hd = j.header(&[0; 12], j.time_start, 0x0102_0304);
        assert_eq!(&hd[0..4], &1u32.to_le_bytes());
        assert_eq!(&hd[4..36], &[3u8; 32]);
        assert_eq!(&hd[76..80], &[4, 3, 2, 1]);
        // body root = H("body", LE32(n), fold(leaf, branch), shares_root)
        let mut cb = vec![1, 2, 3];
        cb.extend_from_slice(&[0; 12]);
        cb.extend_from_slice(&[9; 300]);
        let leaf = h(b"leaf", &[blake3::hash(&cb).as_bytes()]);
        let n1 = h(b"node", &[&leaf, &[5; 32]]);
        let n2 = h(b"node", &[&n1, &[6; 32]]);
        assert_eq!(&hd[36..68], &h(b"body", &[&3u32.to_le_bytes(), &n2, &[8; 32]]));
    }

    #[test]
    fn frames_parse() {
        let f = share(5, 9);
        let (t, r, c) = parse_frame(&f).unwrap().unwrap();
        assert_eq!((t, c), (MSG_SHARE, f.len()));
        assert_eq!(decode_share(&f[r]).unwrap(), (5, 9));
        assert!(parse_frame(&f[..f.len() - 1]).unwrap().is_none());
        let s = submit(1, &[0; 12], 77, 88);
        let (_, r, _) = parse_frame(&s).unwrap().unwrap();
        assert_eq!(decode_submit(&s[r]).unwrap(), (1, vec![0; 12], 77, 88));
        assert!(parse_frame(&[0, 0, 0, 0]).is_err());
        assert!(parse_frame(&[1, 0, 0, 0, 9]).is_err());
        let hl = hello("ffgn");
        let (_, r, _) = parse_frame(&hl).unwrap().unwrap();
        assert_eq!(decode_hello(&hl[r]).unwrap(), (1, "ffgn".to_string()));
    }

    #[test]
    fn targets() {
        let mut t = [0u8; 32];
        t[31] = 1;
        let mut x = [0xffu8; 32];
        x[31] = 0;
        assert!(admits(&t, &x));
        x[31] = 1; // top bytes equal, lower bytes above the target's zeros
        assert!(!admits(&t, &x));
        x[..31].fill(0);
        assert!(admits(&t, &x)); // equal
        let mut y = [0u8; 32];
        y[31] = 2;
        assert!(!admits(&t, &y));
        assert_eq!(minus_one(&[0; 32]), None);
        let mut one = [0u8; 32];
        one[1] = 1;
        let mut want = [0u8; 32];
        want[0] = 0xff;
        assert_eq!(minus_one(&one), Some(want));
    }
}
