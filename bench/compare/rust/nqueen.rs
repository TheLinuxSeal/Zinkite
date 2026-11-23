// plb2 nqueen: counts the solutions of the n-queens problem with an iterative
// backtracking search over bit masks, after the second C implementation on
// Rosetta Code. Small fixed-size state, branchy integer code, no memory traffic.
mod bench;
use bench::*;

const N: i64 = 12;                       // BENCH_N

fn nq_solve(n: i32) -> i32 {
    let mut a = [-1i32; 32];             // the column chosen at each row
    let mut l = [0u32; 32];              // the columns, and both diagonals, under attack
    let mut c = [0u32; 32];
    let mut r = [0u32; 32];
    let y0: u32 = (1u32 << n) - 1;
    let mut m = 0;
    let mut k: i32 = 0;
    while k >= 0 {
        let ku = k as usize;
        let y = (l[ku] | c[ku] | r[ku]) & y0;
        if (y ^ y0) >> (a[ku] + 1) != 0 { // a column past the last one tried is free
            let mut i = a[ku] + 1;
            while i < n && y & (1u32 << i) != 0 {
                i += 1;
            }
            if k < n - 1 {
                let z = 1u32 << i;
                a[ku] = i;
                k += 1;
                let ku = k as usize;
                l[ku] = (l[ku - 1] | z) << 1;
                c[ku] = c[ku - 1] | z;
                r[ku] = (r[ku - 1] | z) >> 1;
            } else {
                m += 1;                  // a solution
                k -= 1;
            }
        } else {
            a[ku] = -1;                  // no choice left: backtrack
            k -= 1;
        }
    }
    m
}

fn main() {
    emit(nq_solve(N as i32) as i64);
}
