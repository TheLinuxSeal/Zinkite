// spectralnorm: the Benchmarks Game spectral norm of the infinite matrix
// A(i, j) = 1 / ((i + j)(i + j + 1) / 2 + i + 1), by ten rounds of the power
// method on N x N. It measures a doubly nested loop of divisions and
// multiply-adds over arrays of doubles, with A made on the fly.
#include "bench.h"
#include <cmath>
#include <vector>

static double a(long long i, long long j) {
    return 1.0 / (double)((i + j) * (i + j + 1) / 2 + i + 1);
}

// out = A v
static void times_a(const std::vector<double>& v, std::vector<double>& out) {
    for (size_t i = 0; i < v.size(); i++) {
        double sum = 0.0;
        for (size_t j = 0; j < v.size(); j++) sum += a(i, j) * v[j];
        out[i] = sum;
    }
}

// out = A' v
static void times_at(const std::vector<double>& v, std::vector<double>& out) {
    for (size_t i = 0; i < v.size(); i++) {
        double sum = 0.0;
        for (size_t j = 0; j < v.size(); j++) sum += a(j, i) * v[j];
        out[i] = sum;
    }
}

// out = A' A v
static void times_ata(const std::vector<double>& v, std::vector<double>& out, std::vector<double>& tmp) {
    times_a(v, tmp);
    times_at(tmp, out);
}

int main() {
    const size_t n = BENCH_N;
    std::vector<double> u(n, 1.0), v(n, 0.0), tmp(n, 0.0);
    for (int i = 0; i < 10; i++) {
        times_ata(u, v, tmp);
        times_ata(v, u, tmp);
    }
    double vbv = 0.0, vv = 0.0;
    for (size_t i = 0; i < n; i++) {
        vbv += u[i] * v[i];
        vv += v[i] * v[i];
    }
    printf("%.9f\n", std::sqrt(vbv / vv));
    return 0;
}
