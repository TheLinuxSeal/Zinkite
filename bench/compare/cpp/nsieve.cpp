// nsieve (Benchmarks Game): the sieve of Eratosthenes over 10000 << N,
// 10000 << (N - 1) and 10000 << (N - 2) flags, one byte each. Prints each
// limit and its prime count. A flat array written with a stride: memory bound
// once the flags outgrow the cache.
#include "bench.h"
#include <vector>

static long long nsieve(long long n) {
    std::vector<uint8_t> flags(n, 1);
    long long count = 0;
    for (long long i = 2; i < n; i++) {
        if (flags[i]) {
            count++;
            for (long long j = i << 1; j < n; j += i) flags[j] = 0;
        }
    }
    return count;
}

int main() {
    for (int i = 0; i < 3; i++) {
        long long n = 10000LL << (BENCH_N - i);
        emit(n);
        emit(nsieve(n));
    }
    return 0;
}
