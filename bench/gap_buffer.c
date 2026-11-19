// gap_buffer.c: inserting into the middle of an ordered array, three ways.
//
// plain:  one contiguous array; insert and remove shift the tail with memmove.
// gap:    a classic gap buffer over the elements themselves. Moving the cursor
//         moves elements, so references into it don't stay valid.
// notes:  Zinkite's cursor layout (DESIGN §4.3, §4.5). Elements live in a data
//         array and never move, so references stay valid, and removal holes
//         are reused through a free list kept inside the holes. A gap buffer
//         of 32-bit indices gives the order. Reads take two loads.
// chunked: the same data array, with the index split into chunks of B
//         entries instead of one gap buffer: a two-level tiered vector
//         (Goodrich and Kloss, 1999). Inserts and removals at any position
//         cost O(B + n / B) and indexing stays O(1). Run with B near sqrt(n)
//         and with B four times that.
//
// Every test runs the same operations, from the same random numbers, on all
// three, and checks that they end with the same contents in the same order.
//
// Build: cc -O2 -o gap_buffer bench/gap_buffer.c
//        cc -O2 -DELEM_WORDS=8 -o gap_buffer64 bench/gap_buffer.c   (64-byte elements)

#define _GNU_SOURCE
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>

#ifndef ELEM_WORDS
#define ELEM_WORDS 2
#endif

typedef struct { int64_t key; int64_t pay[ELEM_WORDS - 1]; } E;

static inline E mk(int64_t v) { E e; e.key = v; for (int i = 0; i < ELEM_WORDS - 1; i++) e.pay[i] = v; return e; }

static double now(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec + t.tv_nsec * 1e-9;
}

static uint64_t rng;
static uint64_t rnd(void) {
    rng ^= rng >> 12; rng ^= rng << 25; rng ^= rng >> 27;
    return rng * 0x2545F4914F6CDD1Dull;
}
static uint64_t rnd_below(uint64_t n) { return rnd() % n; }

static void check(int ok, const char *msg) {
    if (!ok) { fprintf(stderr, "check failed: %s\n", msg); exit(1); }
}

static void *xmalloc(size_t n) { void *p = malloc(n); check(p != NULL, "malloc"); return p; }

static volatile uint64_t sink;

// Makes the compiler assume memory changed, so repeated scans aren't merged.
#define BARRIER() __asm__ volatile("" ::: "memory")

// ---------------------------------------------------------------- plain

typedef struct { E *a; size_t n, cap; } Plain;

static void plain_init(Plain *s, size_t n) {
    s->cap = n + n / 8 + 64;        /* spare room: an eighth */
    s->a = xmalloc(s->cap * sizeof(E));
    for (size_t i = 0; i < n; i++) s->a[i] = mk((int64_t)i);
    s->n = n;
}
static void plain_drop(Plain *s) { free(s->a); }
static inline size_t plain_len(Plain *s) { return s->n; }
static inline void plain_insert_at(Plain *s, size_t i, E x) {
    if (s->n == s->cap) { s->cap *= 2; s->a = realloc(s->a, s->cap * sizeof(E)); }
    memmove(s->a + i + 1, s->a + i, (s->n - i) * sizeof(E));
    s->a[i] = x;
    s->n++;
}
static inline void plain_remove_at(Plain *s, size_t i) {
    memmove(s->a + i, s->a + i + 1, (s->n - i - 1) * sizeof(E));
    s->n--;
}
// A burst: k inserts typed at position p, one call each, as user code would.
static inline void plain_insert_burst(Plain *s, size_t p, size_t k, int64_t v) {
    for (size_t j = 0; j < k; j++) plain_insert_at(s, p + j, mk(v + j));
}
static inline void plain_remove_burst(Plain *s, size_t q, size_t k) {
    for (size_t j = 0; j < k; j++) plain_remove_at(s, q);
}
static inline const E *plain_get(Plain *s, size_t i) { return &s->a[i]; }
static uint64_t plain_scan(Plain *s) {
    uint64_t sum = 0;
    for (size_t i = 0; i < s->n; i++) sum += (uint64_t)s->a[i].key ^ i;
    return sum;
}

// ---------------------------------------------------------------- gap

typedef struct { E *b; size_t cap, gs, ge; } Gap;      // the gap is [gs, ge)

static void gap_init(Gap *s, size_t n) {
    s->cap = n + n / 8 + 64;        /* spare room: an eighth */
    s->b = xmalloc(s->cap * sizeof(E));
    for (size_t i = 0; i < n; i++) s->b[i] = mk((int64_t)i);
    s->gs = n;
    s->ge = s->cap;
}
static void gap_drop(Gap *s) { free(s->b); }
static inline size_t gap_len(Gap *s) { return s->cap - (s->ge - s->gs); }
static inline void gap_seek(Gap *s, size_t p) {
    if (p < s->gs) {
        size_t d = s->gs - p;
        memmove(s->b + s->ge - d, s->b + p, d * sizeof(E));
        s->gs -= d; s->ge -= d;
    } else if (p > s->gs) {
        size_t d = p - s->gs;
        memmove(s->b + s->gs, s->b + s->ge, d * sizeof(E));
        s->gs += d; s->ge += d;
    }
}
static __attribute__((noinline)) void gap_grow(Gap *s) {
    size_t cap = s->cap * 2, tail = s->cap - s->ge;
    E *b = xmalloc(cap * sizeof(E));
    memcpy(b, s->b, s->gs * sizeof(E));
    memcpy(b + cap - tail, s->b + s->ge, tail * sizeof(E));
    free(s->b);
    s->b = b; s->ge = cap - tail; s->cap = cap;
}
static inline void gap_insert(Gap *s, E x) { if (s->gs == s->ge) gap_grow(s); s->b[s->gs++] = x; }
static inline void gap_remove_after(Gap *s) { s->ge++; }
static inline void gap_insert_at(Gap *s, size_t i, E x) { gap_seek(s, i); gap_insert(s, x); }
static inline void gap_remove_at(Gap *s, size_t i) { gap_seek(s, i); gap_remove_after(s); }
static inline void gap_insert_burst(Gap *s, size_t p, size_t k, int64_t v) {
    gap_seek(s, p);
    for (size_t j = 0; j < k; j++) gap_insert(s, mk(v + j));
}
static inline void gap_remove_burst(Gap *s, size_t q, size_t k) {
    gap_seek(s, q);
    for (size_t j = 0; j < k; j++) gap_remove_after(s);
}
static inline const E *gap_get(Gap *s, size_t i) {
    return i < s->gs ? &s->b[i] : &s->b[i + (s->ge - s->gs)];
}
static uint64_t gap_scan(Gap *s) {
    uint64_t sum = 0;
    size_t i = 0;
    for (size_t j = 0; j < s->gs; j++, i++) sum += (uint64_t)s->b[j].key ^ i;
    for (size_t j = s->ge; j < s->cap; j++, i++) sum += (uint64_t)s->b[j].key ^ i;
    return sum;
}

// ---------------------------------------------------------------- data array

// Shared by both cursor layouts: elements never move, and a removed element's
// slot goes on a free list stored inside the hole, for the next insert.
#define NONE UINT32_MAX
typedef struct { E *d; size_t dn, reserved; uint32_t free_head; } Data;

static void data_init(Data *a, size_t n, size_t max_slots) {
    a->reserved = (max_slots + 64) * sizeof(E);
    a->d = mmap(NULL, a->reserved, PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
    check(a->d != MAP_FAILED, "mmap");
    for (size_t i = 0; i < n; i++) a->d[i] = mk((int64_t)i);
    a->dn = n;
    a->free_head = NONE;
}
static void data_drop(Data *a) { munmap(a->d, a->reserved); }
static inline uint32_t data_put(Data *a, E x) {
    uint32_t slot;
    if (a->free_head != NONE) { slot = a->free_head; a->free_head = (uint32_t)a->d[slot].key; }
    else slot = (uint32_t)a->dn++;
    a->d[slot] = x;
    return slot;
}
static inline void data_free(Data *a, uint32_t slot) {
    a->d[slot].key = (int64_t)a->free_head;
    a->free_head = slot;
}

// ---------------------------------------------------------------- notes (one gap)

typedef struct {
    Data a;
    uint32_t *ix; size_t cap, gs, ge;      // gap buffer of indices into a.d
} Notes;

static void notes_init(Notes *s, size_t n, size_t max_inserts) {
    data_init(&s->a, n, n + max_inserts);
    s->cap = n + n / 8 + 64;        /* spare room: an eighth */
    s->ix = xmalloc(s->cap * sizeof(uint32_t));
    for (size_t i = 0; i < n; i++) s->ix[i] = (uint32_t)i;
    s->gs = n;
    s->ge = s->cap;
}
static void notes_drop(Notes *s) { data_drop(&s->a); free(s->ix); }
static inline void notes_seek(Notes *s, size_t p) {
    if (p < s->gs) {
        size_t d = s->gs - p;
        memmove(s->ix + s->ge - d, s->ix + p, d * sizeof(uint32_t));
        s->gs -= d; s->ge -= d;
    } else if (p > s->gs) {
        size_t d = p - s->gs;
        memmove(s->ix + s->gs, s->ix + s->ge, d * sizeof(uint32_t));
        s->gs += d; s->ge += d;
    }
}
static __attribute__((noinline)) void notes_grow(Notes *s) {
    size_t cap = s->cap * 2, tail = s->cap - s->ge;
    uint32_t *ix = xmalloc(cap * sizeof(uint32_t));
    memcpy(ix, s->ix, s->gs * sizeof(uint32_t));
    memcpy(ix + cap - tail, s->ix + s->ge, tail * sizeof(uint32_t));
    free(s->ix);
    s->ix = ix; s->ge = cap - tail; s->cap = cap;
}
static inline void notes_insert(Notes *s, E x) {
    if (s->gs == s->ge) notes_grow(s);
    s->ix[s->gs++] = data_put(&s->a, x);
}
static inline void notes_remove_after(Notes *s) { data_free(&s->a, s->ix[s->ge]); s->ge++; }
static inline void notes_insert_at(Notes *s, size_t i, E x) { notes_seek(s, i); notes_insert(s, x); }
static inline void notes_remove_at(Notes *s, size_t i) { notes_seek(s, i); notes_remove_after(s); }
static inline void notes_insert_burst(Notes *s, size_t p, size_t k, int64_t v) {
    notes_seek(s, p);
    for (size_t j = 0; j < k; j++) notes_insert(s, mk(v + j));
}
static inline void notes_remove_burst(Notes *s, size_t q, size_t k) {
    notes_seek(s, q);
    for (size_t j = 0; j < k; j++) notes_remove_after(s);
}
static inline const E *notes_get(Notes *s, size_t i) {
    return &s->a.d[i < s->gs ? s->ix[i] : s->ix[i + (s->ge - s->gs)]];
}
static uint64_t notes_scan(Notes *s) {
    uint64_t sum = 0;
    size_t i = 0;
    for (size_t j = 0; j < s->gs; j++, i++) sum += (uint64_t)s->a.d[s->ix[j]].key ^ i;
    for (size_t j = s->ge; j < s->cap; j++, i++) sum += (uint64_t)s->a.d[s->ix[j]].key ^ i;
    return sum;
}

// ---------------------------------------------------------------- chunked

// Every chunk but the last holds exactly B = 2^L entries in a circular buffer
// starting at head[c], so position i is chunk i >> L, entry (head + i) & M.
static unsigned chunk_extra;    // B is the power of two at or above sqrt(n), times 2^chunk_extra

typedef struct {
    Data a;
    uint32_t *buf, *head;       // chunk c's entries: buf[c << L .. (c + 1) << L)
    size_t nchunks_cap, n;
    unsigned L; uint32_t M;
} Chunked;

#define SLOT(s, c, k) ((s)->buf[((size_t)(c) << (s)->L) + (((s)->head[c] + (uint32_t)(k)) & (s)->M)])

static void chunked_init(Chunked *s, size_t n, size_t max_inserts) {
    data_init(&s->a, n, n + max_inserts);
    size_t r = 1;
    while (r * r < n) r++;
    unsigned L = 0;
    while (((size_t)1 << L) < r) L++;
    s->L = L + chunk_extra < 4 ? 4 : L + chunk_extra;
    s->M = (1u << s->L) - 1;
    s->nchunks_cap = ((n + n / 8 + 64) >> s->L) + 2;
    s->buf = xmalloc((s->nchunks_cap << s->L) * sizeof(uint32_t));
    s->head = calloc(s->nchunks_cap, sizeof(uint32_t));
    for (size_t i = 0; i < n; i++) s->buf[i] = (uint32_t)i;
    s->n = n;
}
static void chunked_drop(Chunked *s) { data_drop(&s->a); free(s->buf); free(s->head); }
static __attribute__((noinline)) void chunked_grow(Chunked *s) {
    size_t cap = s->nchunks_cap * 2;
    s->buf = realloc(s->buf, (cap << s->L) * sizeof(uint32_t));
    s->head = realloc(s->head, cap * sizeof(uint32_t));
    memset(s->head + s->nchunks_cap, 0, (cap - s->nchunks_cap) * sizeof(uint32_t));
    s->nchunks_cap = cap;
}
static inline const E *chunked_get(Chunked *s, size_t i) {
    return &s->a.d[SLOT(s, i >> s->L, i & s->M)];
}
static void chunked_insert_at(Chunked *s, size_t i, E x) {
    if ((s->n >> s->L) + 1 >= s->nchunks_cap) chunked_grow(s);
    uint32_t v = data_put(&s->a, x), B = s->M + 1, o = (uint32_t)(i & s->M);
    size_t c = i >> s->L, last = s->n >> s->L;
    if (c == last) {                                   // the last chunk has room
        uint32_t m = (uint32_t)(s->n & s->M);
        for (uint32_t k = m; k > o; k--) SLOT(s, c, k) = SLOT(s, c, k - 1);
        SLOT(s, c, o) = v;
    } else {
        uint32_t carry = SLOT(s, c, B - 1);           // a full chunk: its last entry moves on
        for (uint32_t k = B - 1; k > o; k--) SLOT(s, c, k) = SLOT(s, c, k - 1);
        SLOT(s, c, o) = v;
        for (size_t d = c + 1; d < last; d++) {       // each full chunk takes it at the front
            uint32_t h = (s->head[d] - 1) & s->M;     //   and gives up its last entry
            uint32_t *p = &s->buf[(d << s->L) + h];
            uint32_t out = *p;
            *p = carry;
            carry = out;
            s->head[d] = h;
        }
        uint32_t h = (s->head[last] - 1) & s->M;       // the last chunk has room at the front
        s->buf[(last << s->L) + h] = carry;
        s->head[last] = h;
    }
    s->n++;
}
static void chunked_remove_at(Chunked *s, size_t i) {
    uint32_t B = s->M + 1, o = (uint32_t)(i & s->M);
    size_t c = i >> s->L, last = (s->n - 1) >> s->L;
    data_free(&s->a, SLOT(s, c, o));
    if (c == last) {
        uint32_t m = (uint32_t)(s->n - (last << s->L));
        for (uint32_t k = o; k + 1 < m; k++) SLOT(s, c, k) = SLOT(s, c, k + 1);
    } else {
        for (uint32_t k = o; k + 1 < B; k++) SLOT(s, c, k) = SLOT(s, c, k + 1);
        for (size_t d = c + 1; d <= last; d++) {      // each later chunk gives its front
            uint32_t h = s->head[d];                  //   to the end of the chunk before
            uint32_t front = s->buf[(d << s->L) + h];
            s->head[d] = (h + 1) & s->M;
            SLOT(s, d - 1, B - 1) = front;
        }
    }
    s->n--;
}
static inline void chunked_insert_burst(Chunked *s, size_t p, size_t k, int64_t v) {
    for (size_t j = 0; j < k; j++) chunked_insert_at(s, p + j, mk(v + j));
}
static inline void chunked_remove_burst(Chunked *s, size_t q, size_t k) {
    for (size_t j = 0; j < k; j++) chunked_remove_at(s, q);
}
static uint64_t chunked_scan(Chunked *s) {
    uint64_t sum = 0;
    size_t i = 0;
    for (size_t c = 0; i < s->n; c++) {
        size_t m = s->n - i < (size_t)s->M + 1 ? s->n - i : (size_t)s->M + 1;
        uint32_t *base = &s->buf[c << s->L], h = s->head[c];
        for (size_t k = 0; k < m; k++, i++) sum += (uint64_t)s->a.d[base[(h + k) & s->M]].key ^ i;
    }
    return sum;
}

// ---------------------------------------------------------------- tests

enum { T_RANDOM, T_BURST, T_LOCAL, T_SCAN, T_READ, TESTS };
static const char *test_names[TESTS] = {
    "insert + remove at random positions",
    "bursts of 16 at random positions",
    "bursts of 16 near the last edit",
    "scan in order, per element",
    "read a[i] at random i",
};

static uint32_t *read_ix;
#define READS (1u << 22)

// The same test code for all three layouts.
#define DEFINE_TESTS(P, S)                                                         \
    static void P##_run(size_t n, size_t ops, double *t, uint64_t *sums,           \
                        double *holes) {                                           \
        S s;                                                                       \
        int64_t v = (int64_t)n;                                                    \
        size_t bursts = ops / 16;                                                  \
        /* random single edits */                                                  \
        P##_init_n(&s, n, ops);                                                    \
        rng = 0x9E3779B97F4A7C15ull;                                               \
        double t0 = now();                                                         \
        for (size_t k = 0; k < ops; k++) {                                         \
            P##_insert_at(&s, rnd_below(n + 1), mk(v++));                          \
            P##_remove_at(&s, rnd_below(n + 1));                                   \
        }                                                                          \
        t[T_RANDOM] = (now() - t0) / ops;                                          \
        sums[T_RANDOM] = P##_scan(&s);                                             \
        /* scan and random reads, on the state the random edits left */           \
        size_t reps = 50000000 / n + 1;                                            \
        uint64_t acc = 0;                                                          \
        t0 = now();                                                                \
        for (size_t r = 0; r < reps; r++) { BARRIER(); acc += P##_scan(&s); }      \
        t[T_SCAN] = (now() - t0) / ((double)reps * n);                             \
        sink = acc;                                                                \
        acc = 0;                                                                   \
        t0 = now();                                                                \
        for (uint32_t k = 0; k < READS; k++) acc += (uint64_t)P##_get(&s, read_ix[k])->key; \
        t[T_READ] = (now() - t0) / READS;                                          \
        sums[T_READ] = acc;                                                        \
        *holes = P##_holes(&s, n);                                                 \
        P##_drop(&s);                                                              \
        /* bursts at random positions: type 16, delete 16 somewhere else */       \
        P##_init_n(&s, n, ops);                                                    \
        rng = 0x9E3779B97F4A7C16ull;                                               \
        t0 = now();                                                                \
        for (size_t b = 0; b < bursts; b++) {                                      \
            P##_insert_burst(&s, rnd_below(n + 1), 16, v); v += 16;                \
            P##_remove_burst(&s, rnd_below(n + 1), 16);                            \
        }                                                                          \
        t[T_BURST] = (now() - t0) / (bursts * 32.0);                               \
        sums[T_BURST] = P##_scan(&s);                                              \
        P##_drop(&s);                                                              \
        /* bursts near the last edit, like typing in an editor */                 \
        P##_init_n(&s, n, ops);                                                    \
        rng = 0x9E3779B97F4A7C17ull;                                               \
        size_t c = n / 2;                                                          \
        P##_insert_burst(&s, c, 0, v);        /* untimed: put the cursor there */  \
        t0 = now();                                                                \
        for (size_t b = 0; b < bursts; b++) {                                      \
            c = step(c, n);                                                        \
            P##_insert_burst(&s, c, 16, v); v += 16;                               \
            c = step(c, n);                                                        \
            P##_remove_burst(&s, c, 16);                                           \
        }                                                                          \
        t[T_LOCAL] = (now() - t0) / (bursts * 32.0);                               \
        sums[T_LOCAL] = P##_scan(&s);                                              \
        P##_drop(&s);                                                              \
    }

// Moves the cursor up to 100 positions either way, staying in [0, n].
static inline size_t step(size_t c, size_t n) {
    int64_t x = (int64_t)c + (int64_t)rnd_below(201) - 100;
    if (x < 0) x = 0;
    if (x > (int64_t)n) x = (int64_t)n;
    return (size_t)x;
}

static void plain_init_n(Plain *s, size_t n, size_t ops) { (void)ops; plain_init(s, n); }
static void gap_init_n(Gap *s, size_t n, size_t ops) { (void)ops; gap_init(s, n); }
static void notes_init_n(Notes *s, size_t n, size_t ops) { notes_init(s, n, ops + 64); }
static void chunked_init_n(Chunked *s, size_t n, size_t ops) { chunked_init(s, n, ops + 64); }
static double plain_holes(Plain *s, size_t n) { (void)s; (void)n; return 0; }
static double gap_holes(Gap *s, size_t n) { (void)s; (void)n; return 0; }
static double notes_holes(Notes *s, size_t n) { return (double)s->a.dn / n; }
static double chunked_holes(Chunked *s, size_t n) { return (double)s->a.dn / n; }

DEFINE_TESTS(plain, Plain)
DEFINE_TESTS(gap, Gap)
DEFINE_TESTS(notes, Notes)
DEFINE_TESTS(chunked, Chunked)

// Logical order unrelated to storage order, as after many edits spread over
// the whole array: the index is a random permutation.
static double notes_scattered_scan(size_t n) {
    Notes s;
    notes_init(&s, n, 0);
    for (size_t i = n - 1; i > 0; i--) {
        size_t j = rnd_below(i + 1);
        uint32_t t = s.ix[i]; s.ix[i] = s.ix[j]; s.ix[j] = t;
    }
    size_t reps = 50000000 / n + 1;
    uint64_t acc = 0;
    double t0 = now();
    for (size_t r = 0; r < reps; r++) { BARRIER(); acc += notes_scan(&s); }
    double t = (now() - t0) / ((double)reps * n);
    sink = acc;
    notes_drop(&s);
    return t;
}

static void fmt(double ns) {
    if (ns >= 1e6) printf(" %12.2f ms", ns / 1e6);
    else if (ns >= 1e3) printf(" %12.2f us", ns / 1e3);
    else printf(" %12.1f ns", ns);
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    size_t sizes[] = {1000, 10000, 100000, 1000000};
    size_t nsizes = 4, only = argc > 1 ? (size_t)atol(argv[1]) : 0;
    read_ix = xmalloc(READS * sizeof(uint32_t));
    printf("elements of %zu bytes; time per operation\n", sizeof(E));
    for (size_t z = 0; z < nsizes; z++) {
        size_t n = sizes[z];
        if (only && n != only) continue;
        size_t ops = 400000000 / n;
        if (ops < 400) ops = 400;
        rng = 12345;
        for (uint32_t k = 0; k < READS; k++) read_ix[k] = (uint32_t)rnd_below(n);
        double t[5][TESTS], h[5];
        uint64_t sum[5][TESTS];
        plain_run(n, ops, t[0], sum[0], &h[0]);
        gap_run(n, ops, t[1], sum[1], &h[1]);
        notes_run(n, ops, t[2], sum[2], &h[2]);
        unsigned B[2];
        for (int v = 0; v < 2; v++) {
            chunk_extra = v ? 2 : 0;
            Chunked probe;
            chunked_init(&probe, n, 0);
            B[v] = probe.M + 1;
            chunked_drop(&probe);
            chunked_run(n, ops, t[3 + v], sum[3 + v], &h[3 + v]);
        }
        for (int k = 0; k < TESTS; k++) {
            if (k == T_SCAN) continue;
            for (int l = 1; l < 5; l++) check(sum[0][k] == sum[l][k], test_names[k]);
        }
        rng = 999;
        double scattered = notes_scattered_scan(n);
        char c1[24], c2[24];
        snprintf(c1, sizeof c1, "chunks of %u", B[0]);
        snprintf(c2, sizeof c2, "chunks of %u", B[1]);
        printf("\nn = %zu (%zu edits per test)\n", n, ops);
        printf("%-38s %15s %15s %15s %15s %15s\n", "", "plain array", "gap buffer", "cursor, 1 gap", c1, c2);
        for (int k = 0; k < TESTS; k++) {
            printf("%-38s", test_names[k]);
            for (int l = 0; l < 5; l++) fmt(t[l][k] * 1e9);
            printf("\n");
        }
        printf("%-38s %15s %15s", "scan, if storage order is scattered", "", "");
        fmt(scattered * 1e9);
        printf("\ndata array after the random edits (holes reused): 1 gap %.2f x n, chunks %.2f x n\n",
               h[2], h[3]);
    }
    printf("\nall five layouts ended with the same contents in every test\n");
    return 0;
}
