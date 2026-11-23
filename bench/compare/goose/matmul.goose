// matmul (plb2): the product of two n x n f64 matrices by the i-k-j triple
// loop, each matrix its rows back to back in one array. The entries are small
// integers, so every product and sum is exact and all four languages print the
// same checksums: the centre element and the sum of all of them. Floating-point
// throughput, and whether the inner loop vectorizes. After Goose's
// bench/plb2/goose/matmul.goose (`simd fn`, row slices with length asserts),
// with generated integer entries in place of its formula.

import std;
import rng;

let N = 300;                             // BENCH_N

fn mat_gen(n: i64, r: u64&) -> f64[>..] {
    var a: f64[>..] = [];
    for i in n * n {
        r = xs_next(r);
        a.push((xs_mod(r, 101) - 50) as f64);
    }
    return a;
}

// `simd`: the inner loop vectorizes, and the CPU's widest vectors run it.
simd fn mat_mul(n: i64, a: f64[:], b: f64[:], c: f64[:]) {
    for i in n {
        let lo = i * n;
        let ai = a[lo..lo + n];
        let ci = c[lo..lo + n];
        assert(ai.len == n && ci.len == n);
        for k in n {
            let aik = ai[k];
            let bk = b[k * n..k * n + n];
            assert(bk.len == n);
            for j in n { ci[j] += aik * bk[j]; }
        }
    }
}

fn main() {
    let n = N;
    var r: u64 = 12345;
    let a = mat_gen(n, r);
    let b = mat_gen(n, r);
    var c: f64[>..] = [];
    c.resize(n * n, 0.0);
    mat_mul(n, a, b, c);
    var total = 0.0;
    for x in c { total += x; }
    print(c[(n >> 1) * n + (n >> 1)] as i64);
    print(total as i64);
}
