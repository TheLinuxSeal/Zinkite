// plb2 nqueen: counts the solutions of the n-queens problem with an iterative
// backtracking search over bit masks, after the second C implementation on
// Rosetta Code. Small fixed-size state, branchy integer code, no memory traffic.
#include "bench.h"
#include <array>

static int nq_solve(int n) {
    std::array<int, 32> a;                   // the column chosen at each row
    a.fill(-1);
    std::array<uint32_t, 32> l{}, c{}, r{};  // the columns, and both diagonals, under attack
    const uint32_t y0 = (1u << n) - 1;
    int m = 0, k = 0;
    while (k >= 0) {
        uint32_t y = (l[k] | c[k] | r[k]) & y0;
        if ((y ^ y0) >> (a[k] + 1)) {        // a column past the last one tried is free
            int i = a[k] + 1;
            while (i < n && (y & (1u << i))) ++i;
            if (k < n - 1) {
                uint32_t z = 1u << i;
                a[k++] = i;
                l[k] = (l[k - 1] | z) << 1;
                c[k] = c[k - 1] | z;
                r[k] = (r[k - 1] | z) >> 1;
            } else {
                ++m;                         // a solution
                --k;
            }
        } else {
            a[k--] = -1;                     // no choice left: backtrack
        }
    }
    return m;
}

int main() {
    emit(nq_solve(BENCH_N));
    return 0;
}
