// fib: the naive doubly recursive Fibonacci. It measures function calls and
// nothing else: no memory, no data, two calls and an add per level.

let N = 32;                              // BENCH_N

recursive fn fibonacci(n: i32) -> i32 {
    if n < 2 { return n; }
    return fibonacci(n - 1) + fibonacci(n - 2);
}

fn main() {
    print(fibonacci(N));
}
