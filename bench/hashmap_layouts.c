// hashmap_layouts.c: two one-region hash map layouts, compared (DESIGN §4.14, §17).
//
// direct:  Goose's dictionary. One array of slots, each holding a 32-bit tag
//          (31 bits of the key's hash, plus an occupied bit), the key and the
//          value. Open addressing with linear probing, a power-of-two capacity
//          and a load of at most 2/3. Growing doubles the array in place (it
//          is the top of its data stack) and rehashes in place by
//          cycle-walking, so no second table ever exists. Entries move when
//          it grows, and it iterates in slot order.
// compact: Python's compact dict, kept in one block: the entries in insertion
//          order (hash, key, value), then spare entry room, then an index of
//          32-bit entry numbers. Growing extends the block and rebuilds only
//          the index, after the new spare room, from the stored hashes.
//          Entries never move, and it iterates in insertion order.
//
// Both tables live in their own address-space reservation, standing in for a
// data stack, so growth never copies a table. Both store 32 bits of hash per
// entry, so an entry is the same size in both: 24 bytes for u64 keys, 32 for
// 16-byte string views. Memory is what the kernel reports as resident
// (mincore), so pages a table reserved but never touched don't count.
//
// Every phase runs the same keys in the same order on both tables, and the
// checksums must match.
//
// Build: cc -O2 -o hashmap_layouts bench/hashmap_layouts.c
//        cc -O2 -DSTR_KEYS -o hashmap_layouts_str bench/hashmap_layouts.c

#define _GNU_SOURCE
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

static double now(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec + t.tv_nsec * 1e-9;
}

static void check(int ok, const char *what) {
    if (!ok) { fprintf(stderr, "FAILED: %s\n", what); exit(1); }
}

static uint64_t rng_state = 0x9E3779B97F4A7C15ull;
static uint64_t rnd(void) {                       // splitmix64
    uint64_t z = (rng_state += 0x9E3779B97F4A7C15ull);
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ull;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EBull;
    return z ^ (z >> 31);
}

static inline uint64_t mix64(uint64_t x) {        // murmur3's finalizer
    x ^= x >> 33; x *= 0xff51afd7ed558ccdull;
    x ^= x >> 33; x *= 0xc4ceb9fe1a85ec53ull;
    x ^= x >> 33;
    return x;
}

#ifdef STR_KEYS
typedef struct { const char *p; int64_t len; } Key;   // a 16-byte view, like Zinkite's string
static inline uint64_t hash_key(Key k) {
    uint64_t h = 0x243F6A8885A308D3ull ^ (uint64_t)k.len;
    int64_t i = 0;
    for (; i + 8 <= k.len; i += 8) { uint64_t w; memcpy(&w, k.p + i, 8); h = mix64(h ^ w); }
    uint64_t w = 0;
    memcpy(&w, k.p + i, (size_t)(k.len - i));
    return mix64(h ^ w ^ 0x9E3779B97F4A7C15ull);
}
static inline int key_eq(Key a, Key b) { return a.len == b.len && memcmp(a.p, b.p, (size_t)a.len) == 0; }
#define KEY_NAME "string (8-24 bytes) -> i64"
#else
typedef uint64_t Key;
static inline uint64_t hash_key(Key k) { return mix64(k); }
static inline int key_eq(Key a, Key b) { return a == b; }
#define KEY_NAME "u64 -> i64"
#endif

static void *reserve(size_t bytes) {
    void *p = mmap(NULL, bytes, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
    check(p != MAP_FAILED, "mmap");
    return p;
}

static size_t resident(void *p, size_t bytes) {   // bytes the kernel has committed
    size_t page = (size_t)sysconf(_SC_PAGESIZE), pages = (bytes + page - 1) / page;
    unsigned char *v = malloc(pages);
    check(mincore(p, bytes, v) == 0, "mincore");
    size_t n = 0;
    for (size_t i = 0; i < pages; i++) n += v[i] & 1;
    free(v);
    return n * page;
}

#define RESERVE ((size_t)1 << 31)                 // 2 GB of address space per table

// ---------------------------------------------------------------- direct

typedef struct { uint32_t tag; Key key; int64_t val; } Slot;
typedef struct { Slot *s; uint8_t *mark; uint64_t cap, count; double grow_time; } Direct;

#define OCC 0x80000000u
static inline uint32_t tag_of(uint64_t h) { return OCC | (uint32_t)(h & 0x7fffffff); }

static void d_new(Direct *d) {
    d->s = reserve(RESERVE);
    d->mark = reserve(RESERVE / sizeof(Slot));    // scratch for the in-place rehash
}

static void d_init(Direct *d) {                   // an empty table, in memory that may be reused
    d->cap = 8; d->count = 0; d->grow_time = 0;
    memset(d->s, 0, d->cap * sizeof(Slot));
}

static void d_free(Direct *d) {
    munmap(d->s, RESERVE);
    munmap(d->mark, RESERVE / sizeof(Slot));
}

// Double in place, then rehash in place (as hashbrown's rehash_in_place does).
// mark[i] = 1 means slot i holds an entry not yet moved to its new position.
// A probe skips placed entries and stops at an empty slot or an unplaced one:
// it takes an empty one, swaps with an unplaced one and goes on placing that.
static void d_grow(Direct *d) {
    double t0 = now();
    uint64_t old = d->cap, cap = old * 2, mask = cap - 1;
    Slot *s = d->s;
    uint8_t *m = d->mark;
    memset(s + old, 0, old * sizeof(Slot));
    for (uint64_t i = 0; i < old; i++) m[i] = s[i].tag != 0;
    memset(m + old, 0, old);
    for (uint64_t i = 0; i < old; i++) {
        while (m[i]) {
            uint64_t j = (s[i].tag & 0x7fffffff) & mask;
            while (s[j].tag != 0 && !m[j]) j = (j + 1) & mask;
            if (j == i) { m[i] = 0; break; }
            if (s[j].tag == 0) { s[j] = s[i]; s[i].tag = 0; m[i] = 0; break; }
            Slot t = s[j]; s[j] = s[i]; s[i] = t; m[j] = 0;
        }
    }
    d->cap = cap;
    d->grow_time += now() - t0;
}

static inline int64_t *d_find(Direct *d, Key k) {
    uint64_t h = hash_key(k), mask = d->cap - 1, i = h & mask;
    uint32_t t = tag_of(h);
    for (;;) {
        Slot *s = &d->s[i];
        if (s->tag == 0) return NULL;
        if (s->tag == t && key_eq(s->key, k)) return &s->val;
        i = (i + 1) & mask;
    }
}

static inline int64_t *d_get_or_add(Direct *d, Key k, int64_t init) {
    uint64_t h = hash_key(k), mask = d->cap - 1, i = h & mask;
    uint32_t t = tag_of(h);
    for (;;) {
        Slot *s = &d->s[i];
        if (s->tag == 0) break;
        if (s->tag == t && key_eq(s->key, k)) return &s->val;
        i = (i + 1) & mask;
    }
    if ((d->count + 1) * 3 > d->cap * 2) {
        d_grow(d);
        mask = d->cap - 1; i = h & mask;
        while (d->s[i].tag != 0) i = (i + 1) & mask;
    }
    Slot *s = &d->s[i];
    s->tag = t; s->key = k; s->val = init;
    d->count++;
    return &s->val;
}

static int64_t d_sum(Direct *d) {
    int64_t sum = 0;
    for (uint64_t i = 0; i < d->cap; i++) if (d->s[i].tag) sum += d->s[i].val;
    return sum;
}

// ---------------------------------------------------------------- compact

typedef struct { uint32_t hash; Key key; int64_t val; } Entry;
typedef struct { char *base; Entry *e; uint32_t *ix; uint64_t icap, ecap, count; double grow_time; } Compact;

static void c_place_index(Compact *c) { c->ix = (uint32_t *)(c->base + c->ecap * sizeof(Entry)); }

static void c_new(Compact *c) {
    c->base = reserve(RESERVE);
    c->e = (Entry *)c->base;
}

static void c_init(Compact *c) {                  // an empty table, in memory that may be reused
    c->icap = 8; c->ecap = c->icap * 2 / 3; c->count = 0; c->grow_time = 0;
    c_place_index(c);
    memset(c->ix, 0, c->icap * sizeof(uint32_t));
}

static void c_free(Compact *c) { munmap(c->base, RESERVE); }

// Entries stay put. The index moves past the new spare room and is rebuilt
// from the stored hashes; the old index's bytes become spare entry room.
static void c_grow(Compact *c) {
    double t0 = now();
    c->icap *= 2;
    c->ecap = c->icap * 2 / 3;
    c_place_index(c);
    uint64_t mask = c->icap - 1;
    uint32_t *ix = c->ix;
    memset(ix, 0, c->icap * sizeof(uint32_t));
    for (uint64_t k = 0; k < c->count; k++) {
        uint64_t i = c->e[k].hash & mask;
        while (ix[i]) i = (i + 1) & mask;
        ix[i] = (uint32_t)(k + 1);
    }
    c->grow_time += now() - t0;
}

static inline int64_t *c_find(Compact *c, Key k) {
    uint64_t h = hash_key(k), mask = c->icap - 1, i = h & mask;
    uint32_t h32 = (uint32_t)h;
    for (;;) {
        uint32_t x = c->ix[i];
        if (!x) return NULL;
        Entry *en = &c->e[x - 1];
        if (en->hash == h32 && key_eq(en->key, k)) return &en->val;
        i = (i + 1) & mask;
    }
}

static inline int64_t *c_get_or_add(Compact *c, Key k, int64_t init) {
    uint64_t h = hash_key(k), mask = c->icap - 1, i = h & mask;
    uint32_t h32 = (uint32_t)h;
    for (;;) {
        uint32_t x = c->ix[i];
        if (!x) break;
        Entry *en = &c->e[x - 1];
        if (en->hash == h32 && key_eq(en->key, k)) return &en->val;
        i = (i + 1) & mask;
    }
    if (c->count == c->ecap) {
        c_grow(c);
        mask = c->icap - 1; i = h & mask;
        while (c->ix[i]) i = (i + 1) & mask;
    }
    Entry *en = &c->e[c->count];
    en->hash = h32; en->key = k; en->val = init;
    c->ix[i] = (uint32_t)(++c->count);
    return &en->val;
}

static int64_t c_sum(Compact *c) {
    int64_t sum = 0;
    for (uint64_t k = 0; k < c->count; k++) sum += c->e[k].val;
    return sum;
}

// ---------------------------------------------------------------- harness

typedef struct { double insert, grow, hit, miss, update, iterate; size_t mem; int64_t check; } Result;

static Key *keys, *absent;
static uint32_t *perm;

#ifdef STR_KEYS
static char *text;
// Present keys start with their index in base 26, so they are all distinct.
// Absent keys start with a capital letter, so none of them is present.
static Key make_key(char **at, uint64_t index, int is_absent) {
    int64_t len = 8 + (int64_t)(rnd() % 17);
    Key k = { *at, len };
    for (int64_t i = 0; i < len; i++) (*at)[i] = (char)('a' + rnd() % 26);
    for (int i = 0; i < 6; i++) { (*at)[i] = (char)('a' + index % 26); index /= 26; }
    if (is_absent) (*at)[0] = (char)('A' + rnd() % 26);
    *at += len;
    return k;
}
#endif

static void make_keys(uint64_t n) {
    keys = malloc(n * sizeof(Key));
    absent = malloc(n * sizeof(Key));
    perm = malloc(n * sizeof(uint32_t));
#ifdef STR_KEYS
    text = malloc(2 * n * 24);
    char *at = text;
    for (uint64_t i = 0; i < n; i++) keys[i] = make_key(&at, i, 0);
    for (uint64_t i = 0; i < n; i++) absent[i] = make_key(&at, i, 1);
#else
    for (uint64_t i = 0; i < n; i++) keys[i] = rnd();
    for (uint64_t i = 0; i < n; i++) absent[i] = rnd();
#endif
    for (uint64_t i = 0; i < n; i++) perm[i] = (uint32_t)i;
    for (uint64_t i = n - 1; i > 0; i--) {
        uint64_t j = rnd() % (i + 1);
        uint32_t t = perm[i]; perm[i] = perm[j]; perm[j] = t;
    }
}

static void free_keys(void) {
    free(keys); free(absent); free(perm);
#ifdef STR_KEYS
    free(text);
#endif
}

// One run of every phase. The first build is on fresh pages, and is where
// memory is measured. The timed builds then reuse that memory, as a data stack
// reuses its pages from one scope to the next. Small maps are built and probed
// `inner` times so each phase runs long enough to time. Results are
// nanoseconds per operation.
#define RUN(NAME, T, NEW, INIT, FREE, ADD, FIND, SUM, MEMBYTES)                    \
static Result NAME(uint64_t n, uint64_t inner) {                                   \
    Result r = {0};                                                                 \
    T t;                                                                            \
    NEW(&t);                                                                        \
    INIT(&t);                                                                       \
    for (uint64_t i = 0; i < n; i++) *ADD(&t, keys[i], 0) = (int64_t)i;            \
    r.mem = MEMBYTES;                                                               \
    double grow = 0, t0 = now();                                                    \
    for (uint64_t rep = 0; rep < inner; rep++) {                                    \
        INIT(&t);                                                                   \
        for (uint64_t i = 0; i < n; i++) *ADD(&t, keys[i], 0) = (int64_t)i;        \
        grow += t.grow_time;                                                        \
    }                                                                               \
    r.insert = (now() - t0) / (n * inner) * 1e9;                                    \
    r.grow = grow / (n * inner) * 1e9;                                              \
    check(t.count == n, "distinct keys");                                           \
    int64_t sum = 0;                                                                \
    t0 = now();                                                                     \
    for (uint64_t rep = 0; rep < inner; rep++)                                      \
        for (uint64_t j = 0; j < n; j++) sum += *FIND(&t, keys[perm[j]]);           \
    r.hit = (now() - t0) / (n * inner) * 1e9;                                       \
    int64_t missing = 0;                                                            \
    t0 = now();                                                                     \
    for (uint64_t rep = 0; rep < inner; rep++)                                      \
        for (uint64_t j = 0; j < n; j++) missing += FIND(&t, absent[j]) == NULL;    \
    r.miss = (now() - t0) / (n * inner) * 1e9;                                      \
    check(missing == (int64_t)(n * inner), "absent keys");                          \
    t0 = now();                                                                     \
    for (uint64_t rep = 0; rep < inner; rep++)                                      \
        for (uint64_t j = 0; j < n; j++) (*ADD(&t, keys[perm[j]], 0))++;            \
    r.update = (now() - t0) / (n * inner) * 1e9;                                    \
    t0 = now();                                                                     \
    for (uint64_t rep = 0; rep < inner; rep++) sum += SUM(&t);                      \
    r.iterate = (now() - t0) / (n * inner) * 1e9;                                   \
    r.check = sum;                                                                  \
    FREE(&t);                                                                       \
    return r;                                                                       \
}

RUN(run_direct, Direct, d_new, d_init, d_free, d_get_or_add, d_find, d_sum,
    resident(t.s, t.cap * sizeof(Slot)))
RUN(run_compact, Compact, c_new, c_init, c_free, c_get_or_add, c_find, c_sum,
    resident(t.base, t.ecap * sizeof(Entry) + t.icap * sizeof(uint32_t)))

static int cmp_d(const void *a, const void *b) {
    double x = *(const double *)a, y = *(const double *)b;
    return (x > y) - (x < y);
}

static Result median(Result *r, int k) {
    Result m = r[0];
    double v[16];
#define MED(F) for (int i = 0; i < k; i++) v[i] = r[i].F; qsort(v, k, sizeof(double), cmp_d); m.F = v[k / 2];
    MED(insert) MED(grow) MED(hit) MED(miss) MED(update) MED(iterate)
    return m;
}

static void row(const char *name, Result r, uint64_t n) {
    printf("  %-8s %7.1f %7.1f %7.1f %7.1f %7.1f %7.2f %9.2f MB %6.1f\n", name,
           r.insert, r.grow, r.hit, r.miss, r.update, r.iterate,
           r.mem / 1048576.0, (double)r.mem / n);
}

int main(int argc, char **argv) {
    uint64_t sizes[] = { 1000, 100000, 1000000, 10000000 };
    int nsizes = argc > 1 ? atoi(argv[1]) : 4, runs = 5;
    printf("keys: %s; entry: %zu bytes (direct slot %zu, compact entry %zu)\n",
           KEY_NAME, sizeof(Entry), sizeof(Slot), sizeof(Entry));
    printf("nanoseconds per operation; median of %d runs\n", runs);
    for (int si = 0; si < nsizes; si++) {
        uint64_t n = sizes[si], inner = n < 2000000 ? 2000000 / n : 1;
        make_keys(n);
        Result d[16], c[16];
        for (int k = 0; k < runs; k++) { d[k] = run_direct(n, inner); c[k] = run_compact(n, inner); }
        check(d[0].check == c[0].check, "checksums match");
        printf("\nn = %llu\n", (unsigned long long)n);
        printf("  %-8s %7s %7s %7s %7s %7s %7s %12s %6s\n", "", "insert", "(grow)",
               "hit", "miss", "update", "iter", "resident", "B/key");
        row("direct", median(d, runs), n);
        row("compact", median(c, runs), n);
        free_keys();
    }
    return 0;
}
