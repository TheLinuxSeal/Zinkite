// spectralnorm: the Benchmarks Game spectral norm of the infinite matrix
// A(i, j) = 1 / ((i + j)(i + j + 1) / 2 + i + 1), by ten rounds of the power
// method on N x N. It measures a doubly nested loop of divisions and
// multiply-adds over arrays of doubles, with A made on the fly. Unlike
// bench/plb/spectral-norm, which works on eight rows at a time for the vector
// unit, this is the plain row-by-row loop the other three languages use.

import std;
import math;

let N = 100;                             // BENCH_N

fn a(i: i64, j: i64) -> f64 { 1.0 / ((i + j) * (i + j + 1) / 2 + i + 1) }

// out = A v
fn times_a(v: f64[:], out: f64[:]) {
    for i in v.len {
        var sum = 0.0;
        for j in v.len { sum += a(i, j) * v[j]; }
        out[i] = sum;
    }
}

// out = A' v
fn times_at(v: f64[:], out: f64[:]) {
    for i in v.len {
        var sum = 0.0;
        for j in v.len { sum += a(j, i) * v[j]; }
        out[i] = sum;
    }
}

// out = A' A v
fn times_ata(v: f64[:], out: f64[:], tmp: f64[:]) {
    times_a(v, tmp);
    times_at(tmp, out);
}

fn main() {
    var u: f64[>..] = [];
    var v: f64[>..] = [];
    var tmp: f64[>..] = [];
    u.resize(N, 1.0);
    v.resize(N, 0.0);
    tmp.resize(N, 0.0);
    for i in 10 {
        times_ata(u, v, tmp);
        times_ata(v, u, tmp);
    }
    var vbv = 0.0;
    var vv = 0.0;
    for i in N {
        vbv += u[i] * v[i];
        vv += v[i] * v[i];
    }
    var line: u8[>..] = [];
    format_flt(line, sqrt(vbv / vv), 9);
    print(line);
}
