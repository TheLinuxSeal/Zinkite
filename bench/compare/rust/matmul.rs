// matmul (plb2): the product of two n x n f64 matrices by the i-k-j triple
// loop, each matrix its rows back to back in one array. The entries are small
// integers, so every product and sum is exact and all four languages print the
// same checksums: the centre element and the sum of all of them. Floating-point
// throughput, and whether the inner loop vectorizes.
mod bench;
use bench::*;

const N: i64 = 300;                      // BENCH_N

fn main() {
    let n = N as usize;
    let mut r: u64 = 12345;
    let mut gen = || {
        r = xs_next(r);
        (xs_mod(r, 101) - 50) as f64
    };
    let a: Vec<f64> = (0..n * n).map(|_| gen()).collect();
    let b: Vec<f64> = (0..n * n).map(|_| gen()).collect();
    let mut c = vec![0.0f64; n * n];
    for (ci, ai) in c.chunks_exact_mut(n).zip(a.chunks_exact(n)) {
        for (&aik, bk) in ai.iter().zip(b.chunks_exact(n)) {
            for (cij, &bkj) in ci.iter_mut().zip(bk) {
                *cij += aik * bkj;
            }
        }
    }
    let total: f64 = c.iter().sum();
    emit(c[(n / 2) * n + n / 2] as i64);
    emit(total as i64);
}
