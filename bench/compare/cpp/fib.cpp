// fib: the naive doubly recursive Fibonacci. It measures function calls and
// nothing else: no memory, no data, two calls and an add per level.
#include "bench.h"

static int32_t fib(int32_t n) {
    if (n < 2) return n;
    return fib(n - 1) + fib(n - 2);
}

int main() {
    emit(fib(BENCH_N));
    return 0;
}
