// fannkuch-redux (Benchmarks Game): for every permutation of 0..n-1, count the
// prefix reversals ("pancake flips") until 0 comes first. Prints the checksum
// and the largest flip count. Small arrays, tight loops, indexing and swaps.
// The plain single-threaded algorithm, the same in all four languages.
mod bench;
use bench::*;

const N: i64 = 10;                       // BENCH_N

fn main() {
    let n = N as usize;
    let mut perm = vec![0usize; n];
    let mut perm1: Vec<usize> = (0..n).collect();
    let mut count = vec![0usize; n];
    let mut max_flips: i64 = 0;
    let mut perm_count: i64 = 0;
    let mut checksum: i64 = 0;
    let mut r = n;
    loop {
        while r != 1 {
            count[r - 1] = r;
            r -= 1;
        }
        perm.copy_from_slice(&perm1);
        let mut flips: i64 = 0;
        let mut k = perm[0];
        while k != 0 {
            perm[..=k].reverse();
            flips += 1;
            k = perm[0];
        }
        max_flips = max_flips.max(flips);
        checksum += if perm_count % 2 == 0 { flips } else { -flips };
        // The next permutation: rotate the first r + 1 elements left by one,
        // and each counter that runs out brings one more element in.
        loop {
            if r == n {
                emit(checksum);
                emit(max_flips);
                return;
            }
            let perm0 = perm1[0];
            for i in 0..r {
                perm1[i] = perm1[i + 1];
            }
            perm1[r] = perm0;
            count[r] -= 1;
            if count[r] > 0 {
                break;
            }
            r += 1;
        }
        perm_count += 1;
    }
}
