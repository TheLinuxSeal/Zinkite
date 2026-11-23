// fannkuch-redux (Benchmarks Game): for every permutation of 0..n-1, count the
// prefix reversals ("pancake flips") until 0 comes first. Prints the checksum
// and the largest flip count. Small arrays, tight loops, indexing and swaps.
// The plain single-threaded algorithm, the same in all four languages.
#include "bench.h"
#include <algorithm>
#include <vector>

int main() {
    const int n = BENCH_N;
    std::vector<int> perm(n), perm1(n), count(n);
    for (int i = 0; i < n; i++) perm1[i] = i;
    int max_flips = 0;
    long long perm_count = 0, checksum = 0;
    int r = n;
    for (;;) {
        while (r != 1) {
            count[r - 1] = r;
            r--;
        }
        perm = perm1;
        int flips = 0;
        for (int k = perm[0]; k != 0; k = perm[0]) {
            std::reverse(perm.begin(), perm.begin() + k + 1);
            flips++;
        }
        max_flips = std::max(max_flips, flips);
        checksum += perm_count % 2 == 0 ? flips : -flips;
        // The next permutation: rotate the first r + 1 elements left by one,
        // and each counter that runs out brings one more element in.
        for (;;) {
            if (r == n) {
                emit(checksum);
                emit(max_flips);
                return 0;
            }
            int perm0 = perm1[0];
            for (int i = 0; i < r; i++) perm1[i] = perm1[i + 1];
            perm1[r] = perm0;
            if (--count[r] > 0) break;
            r++;
        }
        perm_count++;
    }
}
