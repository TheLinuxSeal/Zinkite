/* The Zinkite runtime (DESIGN §12.2), included at the top of every generated C file.
 * Data stacks, output, failures, checked arithmetic, and the core of the library. */

#include <errno.h>
#include <math.h>
#include <signal.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#define ZR_NORETURN __attribute__((noreturn))
#define ZR_UNUSED __attribute__((unused))

/* A slice or a view: `T[:]`, `T[:]&`, and `string`, which is `u8[:]`. */
typedef struct { void *p; int64_t n; } zr_slice;

/* An array on a data stack: its elements start at p. */
typedef struct { void *p; int64_t n; } zr_vec;

/* ------------------------------------------------------------ failures (DESIGN §7.6) */

static void zr_flush(void);

static ZR_NORETURN ZR_UNUSED void zr_fail(const char *file, int line, const char *msg) {
    zr_flush();
    fprintf(stderr, "%s:%d: %s\n", file, line, msg);
    exit(1);
}

static ZR_NORETURN ZR_UNUSED void zr_fail_text(const char *file, int line, zr_slice msg) {
    zr_flush();
    fprintf(stderr, "%s:%d: %.*s\n", file, line, (int)msg.n, (const char *)msg.p);
    exit(1);
}

/* ------------------------------------------------------------ data stacks (DESIGN §3.1, §11) */

/* Each stack is a reservation of address space; pages are committed as they are touched. */
#define ZR_STACK_BYTES ((size_t)8 << 30)

static void *zr_stacks[4096];

static ZR_UNUSED void *zr_stack(int site) {
    if (zr_stacks[site]) return zr_stacks[site];
    void *p = mmap(NULL, ZR_STACK_BYTES, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
    if (p == MAP_FAILED) zr_fail("zink runtime", 0, "can't reserve memory for a data stack");
    zr_stacks[site] = p;
    return p;
}

/* The scratch stack holds temporaries, such as interpolated text passed to a function,
 * which are released at the end of their statement (DESIGN §4.11). */
static char *zr_scratch_base, *zr_scratch_top;

static ZR_UNUSED int64_t zr_sm(void) {
    if (!zr_scratch_base) zr_scratch_base = zr_scratch_top = (char *)zr_stack(0);
    return zr_scratch_top - zr_scratch_base;
}

static ZR_UNUSED void zr_sr(int64_t m) { zr_scratch_top = zr_scratch_base + m; }

/* ------------------------------------------------------------ output (DESIGN §4.13) */

static char zr_obuf[1 << 16];
static size_t zr_on;
static int zr_out_failed, zr_out_tty = -1;

static void zr_flush(void) {
    size_t off = 0;
    while (off < zr_on) {
        ssize_t w = write(1, zr_obuf + off, zr_on - off);
        if (w < 0) {
            if (errno == EINTR) continue;
            if (errno == EPIPE) exit(141); /* the reader has gone: end quietly (DESIGN §7.6) */
            zr_out_failed = 1;
            break;
        }
        off += (size_t)w;
    }
    zr_on = 0;
}

static void zr_out(int fd, const void *p, size_t n) {
    if (fd == 2) {
        zr_flush();
        while (n > 0) {
            ssize_t w = write(2, p, n);
            if (w < 0) {
                if (errno == EINTR) continue;
                zr_out_failed = 1;
                return;
            }
            p = (const char *)p + w;
            n -= (size_t)w;
        }
        return;
    }
    if (n > sizeof zr_obuf) {
        zr_flush();
        while (n > 0) {
            ssize_t w = write(1, p, n);
            if (w < 0) {
                if (errno == EINTR) continue;
                if (errno == EPIPE) exit(141);
                zr_out_failed = 1;
                return;
            }
            p = (const char *)p + w;
            n -= (size_t)w;
        }
        return;
    }
    if (zr_on + n > sizeof zr_obuf) zr_flush();
    memcpy(zr_obuf + zr_on, p, n);
    zr_on += n;
}

/* Where text goes: a file descriptor, or the end of an array on a data stack. */
typedef struct { int fd; zr_vec *v; } zr_sink;

static ZR_UNUSED zr_sink zr_fd(int fd) { zr_sink s = {fd, NULL}; return s; }
static ZR_UNUSED zr_sink zr_into(zr_vec *v) { zr_sink s = {-1, v}; return s; }

static void zr_put(zr_sink s, const void *p, size_t n) {
    if (s.v) {
        memcpy((char *)s.v->p + s.v->n, p, n);
        s.v->n += (int64_t)n;
    } else {
        zr_out(s.fd, p, n);
    }
}

static ZR_UNUSED void zr_end_line(int fd) {
    zr_out(fd, "\n", 1);
    if (fd == 1) {
        if (zr_out_tty < 0) zr_out_tty = isatty(1);
        if (zr_out_tty) zr_flush();
    }
}

static ZR_UNUSED void zr_w_str(zr_sink s, zr_slice t) { zr_put(s, t.p, (size_t)t.n); }
static ZR_UNUSED void zr_w_byte(zr_sink s, uint8_t b) { zr_put(s, &b, 1); }
static ZR_UNUSED void zr_w_bool(zr_sink s, bool b) { if (b) zr_put(s, "true", 4); else zr_put(s, "false", 5); }

static int zr_fmt_u64(char *buf, uint64_t x) {
    char tmp[24];
    int n = 0;
    do { tmp[n++] = (char)('0' + x % 10); x /= 10; } while (x);
    for (int i = 0; i < n; i++) buf[i] = tmp[n - 1 - i];
    return n;
}

static int zr_fmt_i64(char *buf, int64_t x) {
    if (x < 0) {
        buf[0] = '-';
        return 1 + zr_fmt_u64(buf + 1, (uint64_t)0 - (uint64_t)x);
    }
    return zr_fmt_u64(buf, (uint64_t)x);
}

/* The shortest text that parses back to the same double (STDLIB §2.4): plain digits
 * for exponents from -6 to 20, as in Java and JavaScript, and scientific beyond. */
static int zr_fmt_f64(char *buf, double x) {
    if (isnan(x)) { memcpy(buf, "NaN", 3); return 3; }
    if (isinf(x)) { if (x < 0) { memcpy(buf, "-Infinity", 9); return 9; } memcpy(buf, "Infinity", 8); return 8; }
    if (x == 0) { if (signbit(x)) { memcpy(buf, "-0.0", 4); return 4; } memcpy(buf, "0.0", 3); return 3; }
    int prec = 1;
    for (; prec < 17; prec++) {
        snprintf(buf, 40, "%.*e", prec - 1, x);
        if (strtod(buf, NULL) == x) break;
    }
    snprintf(buf, 40, "%.*e", prec - 1, x);
    int exp = atoi(strchr(buf, 'e') + 1);
    int n;
    if (exp >= -6 && exp < 21) {
        int decimals = prec - 1 - exp;
        if (decimals < 0) decimals = 0;
        n = snprintf(buf, 400, "%.*f", decimals, x);
        if (!strchr(buf, '.')) { buf[n++] = '.'; buf[n++] = '0'; buf[n] = 0; }
    } else {
        /* 1.5e+300 -> 1.5e300, 1e-07 -> 1e-7 */
        char *e = strchr(buf, 'e');
        char *src = e + 1, *dst = e + 1;
        if (*src == '+') src++;
        else if (*src == '-') *dst++ = *src++;
        while (*src == '0' && src[1]) src++;
        while (*src) *dst++ = *src++;
        *dst = 0;
        n = (int)strlen(buf);
    }
    return n;
}

static void zr_pad(zr_sink s, const char *p, int n, int64_t w, char fill) {
    if (fill == '0' && n > 0 && p[0] == '-') {
        zr_put(s, p, 1);
        p++; n--; w--;
    }
    for (int64_t i = n; i < w; i++) zr_put(s, &fill, 1);
    zr_put(s, p, (size_t)n);
}

static ZR_UNUSED void zr_w_i64(zr_sink s, int64_t x) { char b[24]; zr_put(s, b, (size_t)zr_fmt_i64(b, x)); }
static ZR_UNUSED void zr_w_u64(zr_sink s, uint64_t x) { char b[24]; zr_put(s, b, (size_t)zr_fmt_u64(b, x)); }
static ZR_UNUSED void zr_w_f64(zr_sink s, double x) { char b[400]; zr_put(s, b, (size_t)zr_fmt_f64(b, x)); }
static ZR_UNUSED void zr_w_i64_pad(zr_sink s, int64_t x, int64_t w, char fill) { char b[24]; zr_pad(s, b, zr_fmt_i64(b, x), w, fill); }
static ZR_UNUSED void zr_w_u64_pad(zr_sink s, uint64_t x, int64_t w, char fill) { char b[24]; zr_pad(s, b, zr_fmt_u64(b, x), w, fill); }
static ZR_UNUSED void zr_w_f64_pad(zr_sink s, double x, int64_t w, char fill) { char b[400]; zr_pad(s, b, zr_fmt_f64(b, x), w, fill); }

static ZR_UNUSED void zr_w_fixed(zr_sink s, double x, int64_t d) {
    char b[400];
    if (d < 0) d = 0;
    if (d > 100) d = 100;
    int n = snprintf(b, sizeof b, "%.*f", (int)d, x);
    zr_put(s, b, (size_t)n);
}

static ZR_UNUSED void zr_w_radix(zr_sink s, uint64_t x, int bits, int shift) {
    char b[70];
    int n = 0;
    const char *digits = "0123456789abcdef";
    uint64_t mask = ((uint64_t)1 << shift) - 1;
    if (bits < 64) x &= ((uint64_t)1 << bits) - 1;
    do { b[n++] = digits[x & mask]; x >>= shift; } while (x);
    for (int i = n - 1; i >= 0; i--) zr_put(s, &b[i], 1);
}

static ZR_UNUSED void zr_w_utf8(zr_sink s, uint32_t c) {
    uint8_t b[4];
    if (c > 0x10FFFF || (c >= 0xD800 && c <= 0xDFFF)) c = 0xFFFD;
    if (c < 0x80) { b[0] = (uint8_t)c; zr_put(s, b, 1); }
    else if (c < 0x800) { b[0] = (uint8_t)(0xC0 | c >> 6); b[1] = (uint8_t)(0x80 | (c & 0x3F)); zr_put(s, b, 2); }
    else if (c < 0x10000) { b[0] = (uint8_t)(0xE0 | c >> 12); b[1] = (uint8_t)(0x80 | (c >> 6 & 0x3F)); b[2] = (uint8_t)(0x80 | (c & 0x3F)); zr_put(s, b, 3); }
    else { b[0] = (uint8_t)(0xF0 | c >> 18); b[1] = (uint8_t)(0x80 | (c >> 12 & 0x3F)); b[2] = (uint8_t)(0x80 | (c >> 6 & 0x3F)); b[3] = (uint8_t)(0x80 | (c & 0x3F)); zr_put(s, b, 4); }
}

/* Text built on the scratch stack, for a temporary (DESIGN §4.11). */
static ZR_UNUSED zr_vec zr_scratch_begin(void) { zr_sm(); zr_vec v = {zr_scratch_top, 0}; return v; }
static ZR_UNUSED zr_slice zr_scratch_end(zr_vec v) { zr_scratch_top = (char *)v.p + v.n; zr_slice s = {v.p, v.n}; return s; }

/* ------------------------------------------------------------ text */

static ZR_UNUSED zr_slice zr_lit(const char *p, int64_t n) { zr_slice s = {(void *)p, n}; return s; }
static ZR_UNUSED zr_slice zr_view(zr_vec v) { zr_slice s = {v.p, v.n}; return s; }

static ZR_UNUSED bool zr_eq(zr_slice a, zr_slice b, size_t elem) {
    return a.n == b.n && memcmp(a.p, b.p, (size_t)a.n * elem) == 0;
}

static ZR_UNUSED int32_t zr_cmp(zr_slice a, zr_slice b) {
    int64_t n = a.n < b.n ? a.n : b.n;
    int c = memcmp(a.p, b.p, (size_t)n);
    if (c) return c < 0 ? -1 : 1;
    return a.n < b.n ? -1 : a.n > b.n ? 1 : 0;
}

static bool zr_space(uint8_t b) { return b == ' ' || b == '\t' || b == '\n' || b == '\r' || b == '\v' || b == '\f'; }

static ZR_UNUSED zr_slice zr_trim(zr_slice s, int start, int end) {
    const uint8_t *p = (const uint8_t *)s.p;
    int64_t i = 0, j = s.n;
    if (start) while (i < j && zr_space(p[i])) i++;
    if (end) while (j > i && zr_space(p[j - 1])) j--;
    zr_slice r = {(void *)(p + i), j - i};
    return r;
}

static ZR_UNUSED bool zr_starts(zr_slice s, zr_slice x) { return s.n >= x.n && memcmp(s.p, x.p, (size_t)x.n) == 0; }
static ZR_UNUSED bool zr_ends(zr_slice s, zr_slice x) { return s.n >= x.n && memcmp((char *)s.p + s.n - x.n, x.p, (size_t)x.n) == 0; }

static int64_t zr_find_from(zr_slice s, zr_slice x, int64_t from) {
    if (x.n == 0) return from;
    for (int64_t i = from; i + x.n <= s.n; i++)
        if (memcmp((char *)s.p + i, x.p, (size_t)x.n) == 0) return i;
    return -1;
}

typedef struct { int64_t v; uint8_t s; } zr_res_i64;
typedef struct { uint64_t v; uint8_t s; } zr_res_u64;
typedef struct { double v; uint8_t s; } zr_res_f64;

static ZR_UNUSED zr_res_i64 zr_find(zr_slice s, zr_slice x, int64_t from) {
    zr_res_i64 r = {zr_find_from(s, x, from), 0};
    if (r.v < 0) { r.v = 0; r.s = 195; } /* MISSING */
    return r;
}

static ZR_UNUSED zr_res_i64 zr_rfind(zr_slice s, zr_slice x) {
    for (int64_t i = s.n - x.n; i >= 0; i--)
        if (memcmp((char *)s.p + i, x.p, (size_t)x.n) == 0) { zr_res_i64 r = {i, 0}; return r; }
    zr_res_i64 r = {0, 195};
    return r;
}

static ZR_UNUSED int64_t zr_count(zr_slice s, zr_slice x) {
    if (x.n == 0) return s.n + 1;
    int64_t n = 0, i = 0;
    while ((i = zr_find_from(s, x, i)) >= 0) { n++; i += x.n; }
    return n;
}

typedef struct { zr_slice a, b; } zr_pair;

static ZR_UNUSED zr_pair zr_split_first(zr_slice s, zr_slice sep, int last) {
    int64_t i = -1;
    if (last) {
        for (int64_t k = s.n - sep.n; k >= 0; k--)
            if (memcmp((char *)s.p + k, sep.p, (size_t)sep.n) == 0) { i = k; break; }
    } else {
        i = zr_find_from(s, sep, 0);
    }
    zr_pair r;
    if (i < 0) { r.a = s; r.b = zr_lit("", 0); return r; }
    r.a.p = s.p; r.a.n = i;
    r.b.p = (char *)s.p + i + sep.n; r.b.n = s.n - i - sep.n;
    return r;
}

/* Iterators (DESIGN §9.4): each `next` gives 0 and a value, or END when nothing is left. */
typedef struct { zr_slice s; int64_t pos; zr_slice sep; int kind; } zr_iter;

static ZR_UNUSED zr_iter zr_iter_new(zr_slice s, int kind, zr_slice sep) { zr_iter it = {s, 0, sep, kind}; return it; }

/* kind 0: lines; 1: words; 2: splitBy(sep). */
static ZR_UNUSED uint8_t zr_iter_next(zr_iter *it, zr_slice *out) {
    const uint8_t *p = (const uint8_t *)it->s.p;
    int64_t n = it->s.n;
    if (it->kind == 0) {
        if (it->pos >= n) return 194;
        int64_t i = it->pos;
        while (i < n && p[i] != '\n') i++;
        int64_t end = i;
        if (end > it->pos && p[end - 1] == '\r') end--;
        out->p = (void *)(p + it->pos); out->n = end - it->pos;
        it->pos = i + 1;
        return 0;
    }
    if (it->kind == 1) {
        int64_t i = it->pos;
        while (i < n && zr_space(p[i])) i++;
        if (i >= n) { it->pos = n; return 194; }
        int64_t j = i;
        while (j < n && !zr_space(p[j])) j++;
        out->p = (void *)(p + i); out->n = j - i;
        it->pos = j;
        return 0;
    }
    if (it->pos > n) return 194;
    int64_t i = zr_find_from(it->s, it->sep, it->pos);
    if (i < 0 || it->sep.n == 0) i = n;
    out->p = (void *)(p + it->pos); out->n = i - it->pos;
    it->pos = i + (it->sep.n ? it->sep.n : 1);
    return 0;
}

/* UTF-8 decoding: an invalid sequence gives U+FFFD and decoding goes on (STDLIB §3.5). */
static ZR_UNUSED uint8_t zr_cp_next(zr_iter *it, uint32_t *out) {
    const uint8_t *p = (const uint8_t *)it->s.p;
    int64_t n = it->s.n, i = it->pos;
    if (i >= n) return 194;
    uint8_t b = p[i];
    uint32_t c;
    int len;
    if (b < 0x80) { c = b; len = 1; }
    else if ((b & 0xE0) == 0xC0) { c = b & 0x1F; len = 2; }
    else if ((b & 0xF0) == 0xE0) { c = b & 0x0F; len = 3; }
    else if ((b & 0xF8) == 0xF0) { c = b & 0x07; len = 4; }
    else { it->pos = i + 1; *out = 0xFFFD; return 0; }
    if (i + len > n) { it->pos = i + 1; *out = 0xFFFD; return 0; }
    for (int k = 1; k < len; k++) {
        if ((p[i + k] & 0xC0) != 0x80) { it->pos = i + 1; *out = 0xFFFD; return 0; }
        c = c << 6 | (p[i + k] & 0x3F);
    }
    it->pos = i + len;
    *out = c;
    return 0;
}

/* ------------------------------------------------------------ numbers (DESIGN §9, STDLIB §2.4) */

#define ZR_ADD(T, a, b, f, l) ({ T _r; if (__builtin_add_overflow((T)(a), (T)(b), &_r)) zr_fail(f, l, "integer overflow"); _r; })
#define ZR_SUB(T, a, b, f, l) ({ T _r; if (__builtin_sub_overflow((T)(a), (T)(b), &_r)) zr_fail(f, l, "integer overflow"); _r; })
#define ZR_MUL(T, a, b, f, l) ({ T _r; if (__builtin_mul_overflow((T)(a), (T)(b), &_r)) zr_fail(f, l, "integer overflow"); _r; })
#define ZR_NEG(T, a, f, l) ({ T _r; if (__builtin_sub_overflow((T)0, (T)(a), &_r)) zr_fail(f, l, "integer overflow"); _r; })
#define ZR_DIV(T, a, b, f, l) ({ T _a = (a), _b = (b), _t; if (_b == 0) zr_fail(f, l, "division by zero"); \
    if ((T)-1 < 0 && _b == (T)-1 && __builtin_mul_overflow(_a, _b, &_t)) zr_fail(f, l, "integer overflow"); (T)(_a / _b); })
#define ZR_REM(T, a, b, f, l) ({ T _a = (a), _b = (b); if (_b == 0) zr_fail(f, l, "division by zero"); \
    ((T)-1 < 0 && _b == (T)-1) ? (T)0 : (T)(_a % _b); })

/* Saturating arithmetic (DESIGN §9.2): an overflow gives the nearest bound. */
#define ZR_SATADD(T, a, b, lo, hi) ({ T _a = (a), _b = (b), _r; if (__builtin_add_overflow(_a, _b, &_r)) _r = _b > 0 ? (hi) : (lo); _r; })
#define ZR_SATSUB(T, a, b, lo, hi) ({ T _a = (a), _b = (b), _r; if (__builtin_sub_overflow(_a, _b, &_r)) _r = _b < 0 ? (hi) : (lo); _r; })
#define ZR_SATMUL(T, a, b, lo, hi) ({ T _a = (a), _b = (b), _r; if (__builtin_mul_overflow(_a, _b, &_r)) _r = ((_a < 0) != (_b < 0)) ? (lo) : (hi); _r; })

/* Shifts never fail (DESIGN §9.2): counts past the width give 0, or the sign for `>>` on a signed value. */
#define ZR_SHL(T, U, a, n) ({ int64_t _n = (int64_t)(n); _n < 0 || _n >= (int64_t)(sizeof(T) * 8) ? (T)0 : (T)((U)(a) << _n); })
#define ZR_SHR(T, a, n) ({ int64_t _n = (int64_t)(n); T _a = (a); _n < 0 || _n >= (int64_t)(sizeof(T) * 8) ? (T)((T)-1 < 0 && _a < 0 ? -1 : 0) : (T)(_a >> _n); })

/* Exact comparisons between a signed and an unsigned integer (DESIGN §9.2). */
static ZR_UNUSED int zr_cmp_su(int64_t a, uint64_t b) { return a < 0 ? -1 : (uint64_t)a < b ? -1 : (uint64_t)a > b ? 1 : 0; }

/* A checked index: out of range stops the program (DESIGN §4.10). */
static ZR_UNUSED int64_t zr_ix(int64_t i, int64_t n, const char *f, int l) {
    if (i < 0 || i >= n) {
        char msg[96];
        snprintf(msg, sizeof msg, "index %lld is out of range for length %lld", (long long)i, (long long)n);
        zr_fail(f, l, msg);
    }
    return i;
}

static ZR_UNUSED int64_t zr_ix_wrap(int64_t i, int64_t n, const char *f, int l) {
    if (n == 0) zr_fail(f, l, "index into an empty array");
    int64_t r = i % n;
    return r < 0 ? r + n : r;
}

static ZR_UNUSED int64_t zr_ix_clamp(int64_t i, int64_t n, const char *f, int l) {
    if (n == 0) zr_fail(f, l, "index into an empty array");
    return i < 0 ? 0 : i >= n ? n - 1 : i;
}

static ZR_UNUSED void zr_range(int64_t i, int64_t j, int64_t n, const char *f, int l) {
    if (i < 0 || j > n || i > j) {
        char msg[96];
        snprintf(msg, sizeof msg, "slice %lld:%lld is out of range for length %lld", (long long)i, (long long)j, (long long)n);
        zr_fail(f, l, msg);
    }
}

/* Parsing: a sign, then digits; INVALID or RANGE on failure (STDLIB §2.4). */
static ZR_UNUSED zr_res_i64 zr_parse_i64(zr_slice s, int64_t base) {
    zr_res_i64 r = {0, 22};
    const uint8_t *p = (const uint8_t *)s.p;
    int64_t i = 0, n = s.n;
    int neg = 0;
    if (base < 2 || base > 36 || n == 0) return r;
    if (p[0] == '-' || p[0] == '+') { neg = p[0] == '-'; i = 1; }
    if (i == n) return r;
    uint64_t acc = 0, limit = neg ? (uint64_t)INT64_MAX + 1 : (uint64_t)INT64_MAX;
    for (; i < n; i++) {
        int d = p[i] >= '0' && p[i] <= '9' ? p[i] - '0' : p[i] >= 'a' && p[i] <= 'z' ? p[i] - 'a' + 10 : p[i] >= 'A' && p[i] <= 'Z' ? p[i] - 'A' + 10 : 99;
        if (d >= base) return r;
        if (acc > (limit - (uint64_t)d) / (uint64_t)base) { r.s = 34; return r; }
        acc = acc * (uint64_t)base + (uint64_t)d;
    }
    r.v = neg ? (int64_t)(0 - acc) : (int64_t)acc;
    r.s = 0;
    return r;
}

static ZR_UNUSED zr_res_u64 zr_parse_u64(zr_slice s, int64_t base) {
    zr_res_u64 r = {0, 22};
    const uint8_t *p = (const uint8_t *)s.p;
    int64_t i = 0, n = s.n;
    if (base < 2 || base > 36 || n == 0) return r;
    if (p[0] == '+') i = 1;
    if (i == n) return r;
    uint64_t acc = 0;
    for (; i < n; i++) {
        int d = p[i] >= '0' && p[i] <= '9' ? p[i] - '0' : p[i] >= 'a' && p[i] <= 'z' ? p[i] - 'a' + 10 : p[i] >= 'A' && p[i] <= 'Z' ? p[i] - 'A' + 10 : 99;
        if (d >= base) return r;
        if (acc > (UINT64_MAX - (uint64_t)d) / (uint64_t)base) { r.s = 34; return r; }
        acc = acc * (uint64_t)base + (uint64_t)d;
    }
    r.v = acc;
    r.s = 0;
    return r;
}

static ZR_UNUSED zr_res_f64 zr_parse_f64(zr_slice s) {
    zr_res_f64 r = {0, 22};
    char buf[128];
    if (s.n == 0 || s.n >= (int64_t)sizeof buf) return r;
    memcpy(buf, s.p, (size_t)s.n);
    buf[s.n] = 0;
    for (int64_t i = 0; i < s.n; i++) if (zr_space((uint8_t)buf[i])) return r;
    char *end;
    double v = strtod(buf, &end);
    if (end != buf + s.n) return r;
    r.v = v;
    r.s = 0;
    return r;
}

/* ------------------------------------------------------------ the program's start */

static zr_vec zr_args;

static ZR_UNUSED void zr_start(int argc, char **argv) {
    signal(SIGPIPE, SIG_IGN);
    zr_slice *a = (zr_slice *)zr_stack(1);
    for (int i = 1; i < argc; i++) { a[i - 1].p = argv[i]; a[i - 1].n = (int64_t)strlen(argv[i]); }
    zr_args.p = a;
    zr_args.n = argc - 1;
}

/* At exit: flush, and report a failed write by `print` and the others (DESIGN §6.4). */
static ZR_UNUSED int zr_finish(int status) {
    zr_flush();
    if (zr_out_failed) {
        fprintf(stderr, "zink: writing the output failed\n");
        if (status == 0) status = 1;
    }
    return status;
}

static ZR_NORETURN ZR_UNUSED void zr_exit(uint8_t status) { exit(zr_finish(status)); }

static const char *zr_status_names[256];

static ZR_UNUSED zr_slice zr_status_message(uint8_t s) {
    static char buf[32];
    const char *m = NULL;
    switch (s) {
        case 0: m = "OK"; break;
        case 1: m = "Failed"; break;
        case 192: m = "Nothing to take"; break;
        case 193: m = "No room"; break;
        case 194: m = "End of input"; break;
        case 195: m = "No such key or element"; break;
        case 196: m = "Position out of range"; break;
        case 197: m = "Wrong kind of value"; break;
        default:
            if (s < 192) m = strerror(s);
            else { snprintf(buf, sizeof buf, "Error %d", s); m = buf; }
    }
    (void)zr_status_names;
    return zr_lit(m, (int64_t)strlen(m));
}
