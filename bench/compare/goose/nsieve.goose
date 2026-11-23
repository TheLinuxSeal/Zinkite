// nsieve (Benchmarks Game): the sieve of Eratosthenes over 10000 << N,
// 10000 << (N - 1) and 10000 << (N - 2) flags, one byte each. Prints each
// limit and its prime count. A flat array written with a stride: memory bound
// once the flags outgrow the cache. From Goose's bench/plb/nsieve/1.goose,
// with the size as a constant and the output as plain numbers.

import std;

let N = 10;                              // BENCH_N

fn nsieve(n: i64) -> i64 {
    var flags: bool[>..] = [];
    flags.resize(n, true);
    var count = 0;
    for i in 2..n {
        if flags[i] {
            count++;
            var j = i << 1;
            while j < n { flags[j] = false; j += i; }
        }
    }
    return count;
}

fn main() {
    for i in 3 {
        let n = 10000 << (N - i);
        print(n);
        print(nsieve(n));
    }
}
