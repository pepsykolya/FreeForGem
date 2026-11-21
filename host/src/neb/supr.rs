// FFGN — see LICENSE
//! The stratum dialect of Suprnova's NEB pools (neb.suprnova.cc:7801-7805), found by connecting (16-17.11.2026, testnet):
//!   -> mining.subscribe [agent]
//!   <- {"result": [[["mining.set_difficulty", S], ["mining.notify", S]], EXTRANONCE1 (8 bytes hex), EXTRANONCE2_SIZE (8)]}
//!   -> mining.authorize [ADDRESS.worker, x]       (a second login in the same session is accepted: the dev fee)
//!   <- mining.set_difficulty [D]                   share target = 2^256 / D  ("low difficulty share of 3.80" = 2^256 / ticket)
//!   <- mining.set_model [weights_hash hex, slot, packed weights BASE64]
//!   <- mining.notify [job_id, parent, coinbase1, coinbase2, branch[], version, bits, time, clean, height, weights_hash, slot,
//!                     tx_count, shares_root, time_max]        (u32 fields as 8 hex digits, big-endian text)
//!   -> mining.submit [login, job_id, extranonce2 hex, time (8 hex), nonce (8 hex)]
//! coinbase = coinbase1 || extranonce1 || extranonce2 || coinbase2; the header as spec/node.md 7.1.  The byte order of the
//! hashes is confirmed by recomputing the difficulty the pool reported for submitted nonces (`--debug-supr FILE`).
use super::proto::{Job, unhex, unhex32};
use serde_json::Value;

/// u32 field: "00000001" (8 hex digits, big-endian text) or a JSON number
pub fn u32_field(v: &Value) -> Option<u32> {
    if let Some(n) = v.as_u64() {
        return u32::try_from(n).ok();
    }
    let s = v.as_str()?;
    u32::from_str_radix(s, 16).ok()
}

/// share target of a stratum difficulty: 2^256 / d (little-endian bytes), saturating at 2^256 - 1
pub fn target_of_difficulty(d: f64) -> [u8; 32] {
    if !(d > 1.0) {
        return [0xff; 32];
    }
    // 2^256 / d = 2^(256 - e) * (2^e / d) with a 64-bit mantissa
    let e = d.log2().floor() as i32; // d in [2^e, 2^(e+1))
    let m = (2f64.powi(e) / d) * 2f64.powi(63); // in (2^62, 2^63]
    let m = m as u128;
    let shift = 256 - e - 63; // bit position of the mantissa's lowest bit
    let mut out = [0u8; 32];
    for bit in 0..64 {
        if (m >> bit) & 1 == 1 {
            let p = shift + bit;
            if (0..256).contains(&p) {
                out[(p / 8) as usize] |= 1 << (p % 8);
            }
        }
    }
    out
}

pub struct SJob {
    pub job_id: String,
    pub job: Job,
    pub slot: u8,
    pub weights_hash: [u8; 32],
    pub time_hex_len: usize,
}

fn rev(mut a: [u8; 32], r: bool) -> [u8; 32] {
    if r {
        a.reverse();
    }
    a
}

/// `variant` bits (byte order tests): 1 = parent reversed, 2 = shares_root reversed, 4 = branch entries reversed.
pub fn parse_notify(p: &Value, en_size: usize, variant: u32) -> Result<SJob, String> {
    let a = p.as_array().ok_or("notify params are not an array")?;
    let g = |i: usize| a.get(i).ok_or_else(|| format!("notify field {i} missing"));
    let hx = |i: usize| -> Result<Vec<u8>, String> { g(i)?.as_str().and_then(unhex).ok_or_else(|| format!("notify field {i} is not hex")) };
    let h32 = |i: usize| -> Result<[u8; 32], String> { g(i)?.as_str().and_then(unhex32).ok_or_else(|| format!("notify field {i} is not 32 bytes of hex")) };
    let u = |i: usize| -> Result<u32, String> { u32_field(g(i)?).ok_or_else(|| format!("notify field {i} is not a u32")) };
    let branch: Vec<[u8; 32]> = g(4)?
        .as_array()
        .ok_or("branch is not an array")?
        .iter()
        .map(|x| x.as_str().and_then(unhex32).map(|b| rev(b, variant & 4 != 0)).ok_or_else(|| "branch entry is not 32 bytes".to_string()))
        .collect::<Result<_, _>>()?;
    if branch.len() > 4096 {
        return Err("branch too long".into());
    }
    let time = u(7)?;
    let time_max = a.get(14).and_then(u32_field).unwrap_or(time.saturating_add(3600));
    let job = Job {
        id: 0,
        clean: g(8)?.as_bool().unwrap_or(false),
        version: u(5)?,
        parent: rev(h32(1)?, variant & 1 != 0),
        time_start: time,
        time_window: time_max.saturating_sub(time),
        bits: u(6)?,
        target: [0; 32], // the network target is not sent: derived from bits by the caller (compact_target)
        share_target: [0; 32],
        coinbase_prefix: hx(2)?,
        coinbase_suffix: hx(3)?,
        extranonce_size: en_size.min(255) as u8,
        branch,
        tx_count: u(12)?,
        shares_root: rev(h32(13)?, variant & 2 != 0),
        height: g(9)?.as_u64().map(|x| x as u32).or_else(|| u32_field(g(9).ok()?)).ok_or("height")?,
        expiry: time_max,
    };
    let slot = g(11)?.as_u64().ok_or("slot")? as u8;
    let weights_hash = h32(10)?;
    Ok(SJob { job_id: g(0)?.as_str().ok_or("job id")?.to_string(), job, slot, weights_hash, time_hex_len: 8 })
}

/// target of compact bits (spec/node.md 4.1: `bits = (shift << 24) | mantissa`, target = mantissa << shift), little-endian
pub fn compact_target(bits: u32) -> [u8; 32] {
    let shift = bits >> 24;
    let mut out = [0u8; 32];
    if shift > 232 {
        return out;
    }
    let mut acc = ((bits & 0x00ff_ffff) as u64) << (shift % 8);
    let mut idx = (shift / 8) as usize;
    while acc != 0 && idx < 32 {
        out[idx] = (acc & 0xff) as u8;
        acc >>= 8;
        idx += 1;
    }
    out
}

/// set_model: [weights_hash, slot, weights (base64 or hex)] -> (slot, hash, packed)
pub fn parse_model(p: &Value) -> Result<(u8, [u8; 32], Vec<u8>), String> {
    let a = p.as_array().ok_or("set_model params are not an array")?;
    let (h, s, w) = (a.first(), a.get(1), a.get(2));
    // neb-pool/1 order is [slot, hash, weights]; Suprnova's is [hash, slot, weights]
    let (hash, slot) = match (h.and_then(Value::as_str), s.and_then(Value::as_u64), h.and_then(Value::as_u64), s.and_then(Value::as_str)) {
        (Some(hh), Some(ss), _, _) => (hh, ss),
        (_, _, Some(ss), Some(hh)) => (hh, ss),
        _ => return Err("set_model: no slot / weights hash".into()),
    };
    let hash = unhex32(hash).ok_or("set_model: weights hash is not 32 bytes")?;
    let w = w.and_then(Value::as_str).ok_or("set_model: no weights")?;
    let packed = match unhex(w) {
        Some(b) => b,
        None => {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.decode(w).map_err(|e| format!("set_model: weights are neither hex nor base64 ({e})"))?
        }
    };
    if slot > 15 {
        return Err("set_model: slot out of range".into());
    }
    Ok((slot as u8, hash, packed))
}

/// `--debug-supr CAPTURE.json --dataset FILE`: recompute the difficulty Suprnova reported for captured submits under each
/// byte-order variant; the variant whose numbers match is the dialect.
pub fn debug(path: &str, dataset: Option<&str>) -> anyhow::Result<()> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let (file, _) = super::load_dataset(dataset, false)?;
    let en1 = unhex(v["en1"].as_str().unwrap_or("")).unwrap_or_default();
    let (slot, hash, packed) = parse_model(&v["model"]).map_err(anyhow::Error::msg)?;
    let model = super::refc::Model::from_packed(&packed, &hash).map_err(anyhow::Error::msg)?;
    let ds = &file.sets[slot as usize];
    let mut sc = super::refc::Scratch::default();
    println!("model slot {slot} {} hash ok; reported: {}", model.arch.name(), v["res"]);
    for variant in 0..8 {
        let sj = parse_notify(&v["notify"], en1.len() + 8, variant).map_err(anyhow::Error::msg)?;
        let mut line = format!("variant {variant}:");
        for s in v["subs"].as_array().unwrap() {
            let en2 = unhex(s[2].as_str().unwrap()).unwrap();
            let time = u32_field(&s[3]).unwrap();
            let nonce = u32::from_str_radix(s[4].as_str().unwrap(), 16).unwrap();
            let mut en = en1.clone();
            en.extend_from_slice(&en2);
            let hd = sj.job.header(&en, time, 0);
            let t = super::refc::ticket_of(&mut sc, &model, ds, &hd, nonce).map_err(anyhow::Error::msg)?;
            line += &format!(" {:.2}", 2f64.powi(256) / super::proto::to_f64(&t));
        }
        println!("{line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn difficulty_targets() {
        let t = target_of_difficulty(2.0);
        assert_eq!(t[31], 0x80);
        assert!(t[..31].iter().all(|&b| b == 0));
        let t = target_of_difficulty(1.0);
        assert_eq!(t, [0xff; 32]);
        let d = 5e8;
        let x = super::super::proto::to_f64(&target_of_difficulty(d));
        assert!(((2f64.powi(256) / x) / d - 1.0).abs() < 1e-9);
        // genesis of the testnet: 0xd5800000 = 2^236
        let c = compact_target(0xd580_0000);
        assert_eq!(c[29], 0x10);
        assert!(c.iter().enumerate().all(|(i, &b)| i == 29 || b == 0));
    }
}
