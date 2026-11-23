// mandelbrot: the Benchmarks Game Mandelbrot set, an N x N bitmap with up to
// 50 iterations per pixel, packed eight pixels to a byte. It measures a tight
// floating-point loop with an early exit. The program prints a checksum of the
// bitmap instead of the bitmap itself.
#include "bench.h"
#include <vector>

constexpr int MAX_ITER = 50;

int main() {
    const long long n = BENCH_N;
    std::vector<uint8_t> image;
    image.reserve((size_t)(n * ((n + 7) / 8)));
    const double inv = 2.0 / (double)n;
    long long inside = 0;
    for (long long y = 0; y < n; y++) {
        const double ci = (double)y * inv - 1.0;
        unsigned bits = 0;
        int count = 0;
        for (long long x = 0; x < n; x++) {
            const double cr = (double)x * inv - 1.5;
            double zr = 0.0, zi = 0.0, tr = 0.0, ti = 0.0;
            for (int i = 0; i < MAX_ITER && tr + ti <= 4.0; i++) {
                zi = 2.0 * zr * zi + ci;
                zr = tr - ti + cr;
                tr = zr * zr;
                ti = zi * zi;
            }
            bits <<= 1;
            if (tr + ti <= 4.0) {
                bits |= 1;
                inside++;
            }
            if (++count == 8) {
                image.push_back((uint8_t)bits);
                bits = 0;
                count = 0;
            }
        }
        if (count > 0) image.push_back((uint8_t)(bits << (8 - count)));
    }
    // FNV-1a over the bitmap.
    uint64_t h = 14695981039346656037ull;
    for (uint8_t b : image) h = (h ^ b) * 1099511628211ull;
    emit((long long)image.size());
    emit(inside);
    emit_u(h);
    return 0;
}
