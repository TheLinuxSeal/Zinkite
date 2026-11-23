// spectralnorm: the Benchmarks Game spectral norm of the infinite matrix
// A(i, j) = 1 / ((i + j)(i + j + 1) / 2 + i + 1), by ten rounds of the power
// method on N x N. It measures a doubly nested loop of divisions and
// multiply-adds over arrays of doubles, with A made on the fly.
mod bench;
#[allow(unused_imports)]
use bench::*;

const N: i64 = 100;                      // BENCH_N

fn a(i: usize, j: usize) -> f64 {
    1.0 / ((i + j) * (i + j + 1) / 2 + i + 1) as f64
}

// out = A v
fn times_a(v: &[f64], out: &mut [f64]) {
    for (i, o) in out.iter_mut().enumerate() {
        let mut sum = 0.0;
        for (j, &x) in v.iter().enumerate() {
            sum += a(i, j) * x;
        }
        *o = sum;
    }
}

// out = A' v
fn times_at(v: &[f64], out: &mut [f64]) {
    for (i, o) in out.iter_mut().enumerate() {
        let mut sum = 0.0;
        for (j, &x) in v.iter().enumerate() {
            sum += a(j, i) * x;
        }
        *o = sum;
    }
}

// out = A' A v
fn times_ata(v: &[f64], out: &mut [f64], tmp: &mut [f64]) {
    times_a(v, tmp);
    times_at(tmp, out);
}

fn main() {
    let n = N as usize;
    let mut u = vec![1.0; n];
    let mut v = vec![0.0; n];
    let mut tmp = vec![0.0; n];
    for _ in 0..10 {
        times_ata(&u, &mut v, &mut tmp);
        times_ata(&v, &mut u, &mut tmp);
    }
    let mut vbv = 0.0;
    let mut vv = 0.0;
    for i in 0..n {
        vbv += u[i] * v[i];
        vv += v[i] * v[i];
    }
    println!("{:.9}", (vbv / vv).sqrt());
}
