// base64: encode N random bytes (RFC 4648, with padding) and decode the text
// back, each 16 times into a new buffer. C++ has no codec in its standard
// library, so this is the usual hand-written one: a table each way, output
// reserved and appended to. Prints the text's length and FNV-1a hash, a sum
// over every pass, and whether the round trip gave the input back.
#include "bench.h"
#include <array>
#include <optional>
#include <span>
#include <string>
#include <string_view>
#include <vector>

static constexpr std::string_view kAlphabet =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

static std::string encode(std::span<const uint8_t> in) {
    std::string out;
    out.reserve((in.size() + 2) / 3 * 4);
    size_t i = 0;
    for (; i + 3 <= in.size(); i += 3) {
        uint32_t n = uint32_t(in[i]) << 16 | uint32_t(in[i + 1]) << 8 | in[i + 2];
        out += kAlphabet[n >> 18];
        out += kAlphabet[(n >> 12) & 63];
        out += kAlphabet[(n >> 6) & 63];
        out += kAlphabet[n & 63];
    }
    if (size_t rest = in.size() - i; rest > 0) {
        uint32_t n = uint32_t(in[i]) << 16 | (rest == 2 ? uint32_t(in[i + 1]) << 8 : 0);
        out += kAlphabet[n >> 18];
        out += kAlphabet[(n >> 12) & 63];
        out += rest == 2 ? kAlphabet[(n >> 6) & 63] : '=';
        out += '=';
    }
    return out;
}

static std::optional<std::vector<uint8_t>> decode(std::string_view text) {
    static constexpr auto table = [] {
        std::array<int8_t, 256> t{};
        t.fill(-1);
        for (int i = 0; i < 64; i++) t[uint8_t(kAlphabet[i])] = int8_t(i);
        return t;
    }();
    if (text.size() % 4 != 0) return std::nullopt;
    size_t pad = 0;
    if (!text.empty() && text.back() == '=') pad = text[text.size() - 2] == '=' ? 2 : 1;
    std::vector<uint8_t> out;
    out.reserve(text.size() / 4 * 3);
    for (size_t i = 0; i < text.size(); i += 4) {
        bool last = i + 4 == text.size();
        int a = table[uint8_t(text[i])], b = table[uint8_t(text[i + 1])];
        int c = last && pad == 2 ? 0 : table[uint8_t(text[i + 2])];
        int d = last && pad >= 1 ? 0 : table[uint8_t(text[i + 3])];
        if ((a | b | c | d) < 0) return std::nullopt;
        uint32_t n = uint32_t(a) << 18 | uint32_t(b) << 12 | uint32_t(c) << 6 | uint32_t(d);
        out.push_back(uint8_t(n >> 16));
        if (!(last && pad == 2)) out.push_back(uint8_t(n >> 8));
        if (!(last && pad >= 1)) out.push_back(uint8_t(n));
    }
    return out;
}

int main() {
    const long long n = BENCH_N, tries = 16;
    std::vector<uint8_t> input;
    input.reserve(n);
    uint64_t r = 12345;
    for (long long i = 0; i < n; i++) {
        r = xs_next(r);
        input.push_back(uint8_t(xs_mod(r, 256)));
    }
    const std::string text = encode(input);
    uint64_t h = 14695981039346656037ull;
    for (unsigned char b : text) h = (h ^ b) * 1099511628211ull;

    // Each pass's buffer is new. One byte of each result goes into the sum, so
    // no pass can be skipped.
    long long encoded = 0;
    for (long long t = 0; t < tries; t++) {
        std::string e = encode(input);
        encoded += (long long)e.size() + (unsigned char)e[t * 1000003 % e.size()];
    }
    long long decoded = 0;
    for (long long t = 0; t < tries; t++) {
        std::vector<uint8_t> d = decode(text).value();
        decoded += (long long)d.size() + d[t * 1000003 % d.size()];
    }
    const std::vector<uint8_t> back = decode(text).value();

    emit((long long)text.size());
    emit_u(h);
    emit(encoded);
    emit(decoded);
    emit(back == input ? 1 : 0);
    return 0;
}
