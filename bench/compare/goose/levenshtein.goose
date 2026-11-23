// levenshtein: the edit distance of every pair of N random words, by
// Wagner-Fischer in two rows as long as the shorter word plus one. It measures
// a tight two-dimensional loop over small arrays, made fresh for each pair.
// The distance function is the one in bench/bddicken/levenshtein.

import rng;
import std;

let N = 1000;                            // BENCH_N

fn levenshtein_distance(s1: u8[:], s2: u8[:]) -> u32 {
    // The shorter string is the column dimension.
    let shorter = if s1.len <= s2.len { s1 } else { s2 };
    let longer = if s1.len <= s2.len { s2 } else { s1 };
    let m = shorter.len;

    // The rows grow on data stacks of their own: making one is a pointer
    // bump, and the call's return takes both back.
    var prev_row: u32[>..] = [];
    var curr_row: u32[>..] = [];
    for i in m + 1 { prev_row.push(i as u32); }
    push_n(curr_row, 0, m + 1);
    var prev: u32[:] = prev_row;
    var curr: u32[:] = curr_row;

    for ch2, j in longer {
        assert(prev.len == m + 1 && curr.len == m + 1);
        curr[0] = (j + 1) as u32;
        for i in m {
            let cost: u32 = if shorter[i] != ch2 { 1 } else { 0 };
            curr[i + 1] = min(min(prev[i + 1] + 1, curr[i] + 1), prev[i] + cost);
        }
        swap(prev, curr);
    }
    return prev[m];
}

fn main() {
    // One text of space-separated random words, then a slice of each word.
    var text: u8[>..] = [];
    var r: u64 = 12345;
    for i in N {
        r = xs_next(r);
        let wl = 3 + xs_mod(r, 18);
        for j in wl {
            r = xs_next(r);
            text.push(('a' + xs_mod(r, 26)) as! u8);
        }
        text.push(' ');
    }
    var words: const u8[:][>..] = [];
    each_split(text, ' ') { if it.len > 0 { words.push(it); } };
    var total = 0;
    var pairs = 0;
    for i in words.len {
        for j in i + 1..words.len {
            total += levenshtein_distance(words[i], words[j]);
            pairs += 1;
        }
    }
    print(pairs);
    print(total);
}
