// fib: the naive doubly recursive Fibonacci. It measures function calls and
// nothing else: no memory, no data, two calls and an add per level.
mod bench;
use bench::*;

const N: i64 = 32;                       // BENCH_N

fn fib(n: i32) -> i32 {
    if n < 2 {
        return n;
    }
    fib(n - 1) + fib(n - 2)
}

fn main() {
    emit(fib(N as i32) as i64);
}
