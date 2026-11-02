//! Identifier, clock and deterministic-randomness helpers.

use sha2::{Digest, Sha256};

pub fn new_id(prefix: &str) -> String {
    let u = uuid::Uuid::new_v4().simple().to_string();
    format!("{}_{}", prefix, &u[..12])
}

pub fn now_utc() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// SplitMix64 finaliser. Deterministic, seedable, dependency-free.
pub fn mix(seed: u64, salt: u64) -> u64 {
    let mut z = seed ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

pub fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Seeded d20 roll used by world observation/skill checks (TDAE/SPDE). Same inputs → same roll.
pub fn d20(seed: i64, salt: &str, n: i64) -> i64 {
    (mix(seed as u64, fnv1a(salt) ^ (n as u64)) % 20) as i64 + 1
}

pub fn pick<'a, T>(seed: i64, salt: &str, n: i64, items: &'a [T]) -> &'a T {
    let i = (mix(seed as u64, fnv1a(salt) ^ (n as u64)) % items.len() as u64) as usize;
    &items[i]
}

/// Lowercase slug suitable for IDs.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if (c == ' ' || c == '-' || c == '_') && !out.ends_with('_') {
            out.push('_');
        }
    }
    let t = out.trim_matches('_').to_string();
    if t.is_empty() { "item".into() } else { t.chars().take(40).collect() }
}

/// Crude token estimate (chars/4). Always labelled an *estimate* in UI and reports.
pub fn estimate_tokens(s: &str) -> i64 {
    ((s.chars().count() as i64) + 3) / 4
}
