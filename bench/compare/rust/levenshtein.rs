// levenshtein: the edit distance of every pair of N random words, by
// Wagner-Fischer in two rows as long as the shorter word plus one. It measures
// a tight two-dimensional loop over small arrays, made fresh for each pair.
mod bench;
use bench::*;

const N: i64 = 1000;                     // BENCH_N

fn distance(s1: &[u8], s2: &[u8]) -> u32 {
    // The shorter word is the row.
    let (a, b) = if s1.len() <= s2.len() { (s1, s2) } else { (s2, s1) };
    let m = a.len();
    let mut prev: Vec<u32> = (0..=m as u32).collect();
    let mut curr: Vec<u32> = vec![0; m + 1];
    for (j, &c) in b.iter().enumerate() {
        curr[0] = (j + 1) as u32;
        for i in 0..m {
            let cost = if a[i] == c { 0 } else { 1 };
            curr[i + 1] = (prev[i + 1] + 1).min(curr[i] + 1).min(prev[i] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[m]
}

fn main() {
    let mut words: Vec<Vec<u8>> = Vec::new();
    let mut r: u64 = 12345;
    for _ in 0..N {
        r = xs_next(r);
        let wl = 3 + xs_mod(r, 18);
        let mut w = Vec::with_capacity(wl as usize);
        for _ in 0..wl {
            r = xs_next(r);
            w.push(b'a' + xs_mod(r, 26) as u8);
        }
        words.push(w);
    }
    let mut total: i64 = 0;
    let mut pairs: i64 = 0;
    for i in 0..words.len() {
        for j in i + 1..words.len() {
            total += distance(&words[i], &words[j]) as i64;
            pairs += 1;
        }
    }
    emit(pairs);
    emit(total);
}
