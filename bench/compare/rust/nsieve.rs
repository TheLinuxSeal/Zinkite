// nsieve (Benchmarks Game): the sieve of Eratosthenes over 10000 << N,
// 10000 << (N - 1) and 10000 << (N - 2) flags, one byte each. Prints each
// limit and its prime count. A flat array written with a stride: memory bound
// once the flags outgrow the cache.
mod bench;
use bench::*;

const N: i64 = 10;                       // BENCH_N

fn nsieve(n: usize) -> i64 {
    let mut flags = vec![true; n];
    let mut count = 0;
    for i in 2..n {
        if flags[i] {
            count += 1;
            let mut j = i << 1;
            while j < n {
                flags[j] = false;
                j += i;
            }
        }
    }
    count
}

fn main() {
    for i in 0..3 {
        let n = 10000i64 << (N - i);
        emit(n);
        emit(nsieve(n as usize));
    }
}
