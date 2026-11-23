// base64: encode N random bytes (RFC 4648, with padding) and decode the text
// back, each 16 times into a new buffer. Rust's standard library has no codec
// (the base64 crate isn't available to a plain rustc build), so this is the
// usual hand-written one: a table each way, output reserved and pushed to.
// Prints the text's length and FNV-1a hash, a sum over every pass, and whether
// the round trip gave the input back.
mod bench;
use bench::*;

const N: i64 = 1000000;                  // BENCH_N
const TRIES: i64 = 16;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn encode(input: &[u8]) -> String {
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    let mut chunks = input.chunks_exact(3);
    for g in &mut chunks {
        let n = (g[0] as u32) << 16 | (g[1] as u32) << 8 | g[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 63) as usize] as char);
        out.push(ALPHABET[(n >> 6 & 63) as usize] as char);
        out.push(ALPHABET[(n & 63) as usize] as char);
    }
    let rest = chunks.remainder();
    if !rest.is_empty() {
        let n = (rest[0] as u32) << 16 | rest.get(1).map_or(0, |&b| (b as u32) << 8);
        out.push(ALPHABET[(n >> 18) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 63) as usize] as char);
        out.push(if rest.len() == 2 { ALPHABET[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push('=');
    }
    out
}

const fn decode_table() -> [i8; 256] {
    let mut t = [-1i8; 256];
    let mut i = 0;
    while i < 64 {
        t[ALPHABET[i] as usize] = i as i8;
        i += 1;
    }
    t
}

static TABLE: [i8; 256] = decode_table();

fn decode(text: &str) -> Option<Vec<u8>> {
    let s = text.as_bytes();
    if s.len() % 4 != 0 {
        return None;
    }
    let pad = if s.last() == Some(&b'=') { if s[s.len() - 2] == b'=' { 2 } else { 1 } } else { 0 };
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let groups = s.len() / 4;
    for (gi, g) in s.chunks_exact(4).enumerate() {
        let last = gi + 1 == groups;
        let a = TABLE[g[0] as usize] as i32;
        let b = TABLE[g[1] as usize] as i32;
        let c = if last && pad == 2 { 0 } else { TABLE[g[2] as usize] as i32 };
        let d = if last && pad >= 1 { 0 } else { TABLE[g[3] as usize] as i32 };
        if (a | b | c | d) < 0 {
            return None;
        }
        let n = (a as u32) << 18 | (b as u32) << 12 | (c as u32) << 6 | d as u32;
        out.push((n >> 16) as u8);
        if !(last && pad == 2) {
            out.push((n >> 8) as u8);
        }
        if !(last && pad >= 1) {
            out.push(n as u8);
        }
    }
    Some(out)
}

fn main() {
    let mut r: u64 = 12345;
    let input: Vec<u8> = (0..N)
        .map(|_| {
            r = xs_next(r);
            xs_mod(r, 256) as u8
        })
        .collect();
    let text = encode(&input);
    let mut h: u64 = 14695981039346656037;
    for &b in text.as_bytes() {
        h = (h ^ b as u64).wrapping_mul(1099511628211);
    }

    // Each pass's buffer is new. One byte of each result goes into the sum, so
    // no pass can be skipped.
    let mut encoded: i64 = 0;
    for t in 0..TRIES {
        let e = encode(&input);
        encoded += e.len() as i64 + e.as_bytes()[(t * 1000003) as usize % e.len()] as i64;
    }
    let mut decoded: i64 = 0;
    for t in 0..TRIES {
        let d = decode(&text).expect("invalid base64");
        decoded += d.len() as i64 + d[(t * 1000003) as usize % d.len()] as i64;
    }
    let back = decode(&text).expect("invalid base64");

    emit(text.len() as i64);
    println!("{}", h);
    emit(encoded);
    emit(decoded);
    emit(if back == input { 1 } else { 0 });
}
