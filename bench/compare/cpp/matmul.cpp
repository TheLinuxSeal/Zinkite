// matmul (plb2): the product of two n x n f64 matrices by the i-k-j triple
// loop, each matrix its rows back to back in one array. The entries are small
// integers, so every product and sum is exact and all four languages print the
// same checksums: the centre element and the sum of all of them. Floating-point
// throughput, and whether the inner loop vectorizes.
#include "bench.h"
#include <vector>

int main() {
    const long long n = BENCH_N;
    uint64_t r = 12345;
    std::vector<double> a(n * n), b(n * n), c(n * n, 0.0);
    for (double& x : a) { r = xs_next(r); x = (double)((long long)xs_mod(r, 101) - 50); }
    for (double& x : b) { r = xs_next(r); x = (double)((long long)xs_mod(r, 101) - 50); }
    for (long long i = 0; i < n; i++) {
        double* ci = &c[i * n];
        for (long long k = 0; k < n; k++) {
            const double aik = a[i * n + k];
            const double* bk = &b[k * n];
            for (long long j = 0; j < n; j++) ci[j] += aik * bk[j];
        }
    }
    double total = 0.0;
    for (double x : c) total += x;
    emit((long long)c[(n / 2) * n + n / 2]);
    emit((long long)total);
    return 0;
}
