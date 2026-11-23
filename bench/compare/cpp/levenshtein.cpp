// levenshtein: the edit distance of every pair of N random words, by
// Wagner-Fischer in two rows as long as the shorter word plus one. It measures
// a tight two-dimensional loop over small arrays, made fresh for each pair.
#include "bench.h"
#include <algorithm>
#include <string>
#include <string_view>
#include <vector>

static uint32_t distance(std::string_view s1, std::string_view s2) {
    // The shorter word is the row.
    std::string_view a = s1.size() <= s2.size() ? s1 : s2;
    std::string_view b = s1.size() <= s2.size() ? s2 : s1;
    size_t m = a.size();
    std::vector<uint32_t> prev(m + 1), curr(m + 1);
    for (size_t i = 0; i <= m; i++) prev[i] = (uint32_t)i;
    for (size_t j = 0; j < b.size(); j++) {
        curr[0] = (uint32_t)(j + 1);
        for (size_t i = 0; i < m; i++) {
            uint32_t cost = a[i] == b[j] ? 0 : 1;
            curr[i + 1] = std::min({prev[i + 1] + 1, curr[i] + 1, prev[i] + cost});
        }
        std::swap(prev, curr);
    }
    return prev[m];
}

int main() {
    std::vector<std::string> words;
    uint64_t r = 12345;
    for (long long i = 0; i < BENCH_N; i++) {
        r = xs_next(r);
        uint64_t wl = 3 + xs_mod(r, 18);
        std::string w;
        for (uint64_t j = 0; j < wl; j++) {
            r = xs_next(r);
            w.push_back((char)('a' + xs_mod(r, 26)));
        }
        words.push_back(std::move(w));
    }
    long long total = 0, pairs = 0;
    for (size_t i = 0; i < words.size(); i++) {
        for (size_t j = i + 1; j < words.size(); j++) {
            total += distance(words[i], words[j]);
            pairs++;
        }
    }
    emit(pairs);
    emit(total);
    return 0;
}
