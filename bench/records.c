// records.c: a typical CLI workload in two memory models.
//
// Parse N lines of "id,name,qty,cents" into records, scan them, sort them by
// name, scan them in sorted order, and free them.
//
// current: Zinkite as DESIGN.md specifies it. Each record is a class object
//   with a 16-byte header (§14.2) and a reference to a separate string object
//   (§12.2). A list<Rec> holds references in a backing array that doubles and
//   counts each element when it is copied (§12.4). Objects come from
//   size-class free lists (§14.3) and are freed one by one, in order, when the
//   list's count reaches zero (§4.3). Counts are updated where §4.4 and §7
//   say they are: stores of parameters are counted, fresh objects are not.
// stacks: the pasted notes' Goose-style model. Records are stored inline and
//   variable-size on one data stack (three ints, a varint length, the name's
//   bytes), with an index of references on a second stack for sorting.
//   Freeing is one store per stack.
//
// Memory modes:
//   fresh:  every round gets newly mapped memory, like one CLI run
//   reused: memory is reused round after round, like a long-running host
//
// Build: cc -O2 -o records bench/records.c

#define _GNU_SOURCE
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>

static double now(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec + t.tv_nsec * 1e-9;
}

static uint64_t rng = 0x9E3779B97F4A7C15ull;
static uint64_t rnd(void) {
    rng ^= rng >> 12; rng ^= rng << 25; rng ^= rng >> 27;
    return rng * 0x2545F4914F6CDD1Dull;
}
static uint64_t rnd_below(uint64_t n) { return rnd() % n; }

static void check(int ok, const char *msg) {
    if (!ok) { fprintf(stderr, "check failed: %s\n", msg); exit(1); }
}

static void *reserve(size_t bytes) {
    void *p = mmap(NULL, bytes, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
    check(p != MAP_FAILED, "mmap");
    return p;
}

static inline uint64_t fnv(uint64_t h, const uint8_t *b, size_t n) {
    for (size_t i = 0; i < n; i++) h = (h ^ b[i]) * 0x100000001B3ull;
    return h;
}

// ---------------------------------------------------------------- input

static char *input;
static size_t input_len;

static void make_input(size_t n) {
    input = malloc(n * 48 + 1);
    char *p = input;
    for (size_t i = 0; i < n; i++) {
        unsigned len = 5 + rnd_below(16);
        p += sprintf(p, "%zu,", i + 1);
        for (unsigned k = 0; k < len; k++) *p++ = 'a' + rnd_below(26);
        p += sprintf(p, ",%u,%u\n", (unsigned)(1 + rnd_below(100)),
                     (unsigned)(1 + rnd_below(100000)));
    }
    input_len = p - input;
}

typedef struct { uint32_t id, qty, cents, len; const char *name; } Fields;

static inline uint32_t parse_uint(const char **pp, char stop) {
    const char *p = *pp;
    uint32_t v = 0;
    while (*p != stop) v = v * 10 + (uint32_t)(*p++ - '0');
    *pp = p + 1;
    return v;
}

// Both models use this parser, so parsing costs the same in each.
static inline const char *parse_line(const char *p, Fields *f) {
    f->id = parse_uint(&p, ',');
    f->name = p;
    while (*p != ',') p++;
    f->len = (uint32_t)(p - f->name);
    p++;
    f->qty = parse_uint(&p, ',');
    f->cents = parse_uint(&p, '\n');
    return p;
}

// Hoare quicksort with median of three, insertion sort below 16 elements.
#define DEFINE_SORT(NAME, T, LESS)                                            \
    static void NAME(T *a, ptrdiff_t n) {                                     \
        while (n > 16) {                                                      \
            T *l = a; T *m = a + n / 2; T *r = a + n - 1; T t;                \
            if (LESS(*m, *l)) { t = *m; *m = *l; *l = t; }                    \
            if (LESS(*r, *m)) { t = *r; *r = *m; *m = t;                      \
                if (LESS(*m, *l)) { t = *m; *m = *l; *l = t; } }              \
            T pivot = *m;                                                     \
            ptrdiff_t i = -1, j = n;                                          \
            for (;;) {                                                        \
                do i++; while (LESS(a[i], pivot));                            \
                do j--; while (LESS(pivot, a[j]));                            \
                if (i >= j) break;                                            \
                t = a[i]; a[i] = a[j]; a[j] = t;                              \
            }                                                                 \
            ptrdiff_t left = j + 1;                                           \
            if (left < n - left) { NAME(a, left); a += left; n -= left; }     \
            else { NAME(a + left, n - left); n = left; }                      \
        }                                                                     \
        for (ptrdiff_t i = 1; i < n; i++) {                                   \
            T x = a[i];                                                       \
            ptrdiff_t j = i;                                                  \
            while (j > 0 && LESS(x, a[j - 1])) { a[j] = a[j - 1]; j--; }      \
            a[j] = x;                                                         \
        }                                                                     \
    }

typedef struct { uint64_t sum, name_bytes; } Scan;

// ---------------------------------------------------------------- current

enum { CLS_STR = 1, CLS_REC, CLS_ARR, CLS_LIST };

typedef struct { uint32_t classid, flags; uint64_t count; } ZHdr;
typedef struct { ZHdr h; uint64_t len; uint8_t bytes[]; } ZStr;           // string
typedef struct { ZHdr h; ZStr *name; int32_t id, qty, cents; } ZRec;      // 40 bytes
typedef struct { ZHdr h; uint64_t len; ZHdr *e[]; } ZArr;                 // array<T?>
typedef struct { ZHdr h; ZArr *data; int32_t size; } ZList;               // list<T>

#define ZSMALL 256
static void *z_free_lists[ZSMALL / 8 + 1];
static char *z_base, *z_bump;
static size_t z_reserved;
static int64_t z_live;

static inline size_t z_round(size_t size) { return (size + 7) & ~(size_t)7; }

static inline void *z_alloc(size_t size) {
    size = z_round(size);
    z_live++;
    if (size > ZSMALL) return calloc(1, size);       // page allocator for big arrays
    void **list = &z_free_lists[size >> 3];
    void *p = *list;
    if (p) { *list = *(void **)p; return p; }
    p = z_bump;
    z_bump += size;
    return p;
}

static inline void z_release(void *p, size_t size) {
    size = z_round(size);
    z_live--;
    if (size > ZSMALL) { free(p); return; }
    void **list = &z_free_lists[size >> 3];
    *(void **)p = *list;
    *list = p;
}

static void z_region_new(size_t bytes) {
    z_reserved = bytes;
    z_base = z_bump = reserve(bytes);
    memset(z_free_lists, 0, sizeof z_free_lists);
}
static void z_region_drop(void) { munmap(z_base, z_reserved); }

// Count updates are real memory updates; volatile stops the C compiler from
// merging an increment with a later decrement, which Zinkite's would not do.
static inline void z_inc(void *o) { ((volatile ZHdr *)o)->count++; }

static size_t z_object_size(ZHdr *o) {
    switch (o->classid) {
    case CLS_STR: return sizeof(ZStr) + ((ZStr *)o)->len;
    case CLS_REC: return sizeof(ZRec);
    case CLS_ARR: return sizeof(ZArr) + ((ZArr *)o)->len * sizeof(ZHdr *);
    default: return sizeof(ZList);
    }
}

// Freeing: depth first, each object's references in declaration order, with an
// explicit stack so long chains can't overflow the C stack (§4.3).
typedef struct { ZHdr *o; uint64_t next; } ZFrame;
static ZFrame *z_frames;
static size_t z_frames_cap;

static __attribute__((noinline)) void z_free(ZHdr *first) {
    size_t top = 0;
    z_frames[top++] = (ZFrame){first, 0};
    while (top) {
        ZFrame *f = &z_frames[top - 1];
        ZHdr *o = f->o, *child = NULL;
        switch (o->classid) {
        case CLS_REC: if (f->next == 0) child = &((ZRec *)o)->name->h; break;
        case CLS_LIST: if (f->next == 0) child = &((ZList *)o)->data->h; break;
        case CLS_ARR: {
            ZArr *a = (ZArr *)o;
            while (f->next < a->len && !a->e[f->next]) f->next++;
            if (f->next < a->len) child = a->e[f->next];
            break;
        }
        }
        if (child) {
            f->next++;
            if (--((volatile ZHdr *)child)->count == 0) {
                if (top == z_frames_cap) {
                    z_frames_cap *= 2;
                    z_frames = realloc(z_frames, z_frames_cap * sizeof *z_frames);
                }
                z_frames[top++] = (ZFrame){child, 0};
            }
        } else {
            top--;
            z_release(o, z_object_size(o));
        }
    }
}

static inline void z_dec(void *o) {
    if (--((volatile ZHdr *)o)->count == 0) z_free(o);
}

static inline ZStr *z_str_new(const char *b, size_t len) {
    ZStr *s = z_alloc(sizeof(ZStr) + len);
    s->h = (ZHdr){CLS_STR, 0, 1};
    s->len = len;
    memcpy(s->bytes, b, len);
    return s;
}

static ZArr *z_arr_new(uint64_t len) {
    size_t size = sizeof(ZArr) + len * sizeof(ZHdr *);
    ZArr *a = z_alloc(size);
    if (z_round(size) <= ZSMALL) memset(a->e, 0, len * sizeof(ZHdr *));  // calloc did the big ones
    a->h = (ZHdr){CLS_ARR, 0, 1};
    a->len = len;
    return a;
}

static ZList *z_list_new(void) {
    ZList *l = z_alloc(sizeof(ZList));
    l->h = (ZHdr){CLS_LIST, 0, 1};
    l->data = z_arr_new(10);                           // fresh: no count update
    l->size = 0;
    return l;
}

// §12.4: growth copies into a new array, counting each element, and the old
// array's free decrements each one again. move=1 models the cheaper version
// where growth hands the references over without counting.
static __attribute__((noinline)) void z_list_grow(ZList *l, int move) {
    ZArr *old = l->data, *grown = z_arr_new(old->len * 2);
    for (int32_t i = 0; i < l->size; i++) {
        grown->e[i] = old->e[i];
        if (!move) z_inc(grown->e[i]);
    }
    l->data = grown;
    if (move) z_release(old, sizeof(ZArr) + old->len * sizeof(ZHdr *));
    else z_dec(old);
}

static inline void z_list_add(ZList *l, ZRec *r, int move) {      // add(T item)
    if ((uint64_t)l->size == l->data->len) z_list_grow(l, move);
    l->data->e[l->size++] = &r->h;
    z_inc(r);                                    // a storable parameter is counted
}

static inline ZRec *z_rec_new(const Fields *f, ZStr *name) {     // new Rec(...)
    ZRec *r = z_alloc(sizeof(ZRec));
    r->h = (ZHdr){CLS_REC, 0, 1};
    r->id = f->id;
    r->qty = f->qty;
    r->cents = f->cents;
    r->name = name;
    z_inc(name);                                 // this.name = name: counted
    return r;
}

//   string name = line.substring(...);
//   recs.add(new Rec(id, name, qty, cents));
static ZList *z_build(int move) {
    ZList *l = z_list_new();
    const char *p = input, *end = input + input_len;
    while (p < end) {
        Fields f;
        p = parse_line(p, &f);
        ZStr *name = z_str_new(f.name, f.len);
        ZRec *r = z_rec_new(&f, name);
        z_list_add(l, r, move);
        z_dec(r);                                // the temporary is released
        z_dec(name);                             // the local is released
    }
    return l;
}

//   for (string line : input.lines()) {
//       array<string> parts = line.split(",");
//       recs.add(new Rec(parseInt(parts[0]), parts[1], ...));
//   }
static ZList *z_build_split(void) {
    ZList *l = z_list_new();
    const char *p = input, *end = input + input_len;
    while (p < end) {
        const char *nl = memchr(p, '\n', end - p);
        ZStr *line = z_str_new(p, nl - p);
        p = nl + 1;
        ZArr *parts = z_arr_new(4);
        const char *b = (const char *)line->bytes, *e = b + line->len;
        for (int k = 0; k < 4; k++) {
            const char *c = k < 3 ? memchr(b, ',', e - b) : e;
            parts->e[k] = &z_str_new(b, c - b)->h;     // fresh: no count update
            b = c + 1;
        }
        Fields f = {0};
        for (int k = 0; k < 4; k += (k == 0 ? 2 : 1)) {
            ZStr *s = (ZStr *)parts->e[k];
            uint32_t v = 0;
            for (uint64_t i = 0; i < s->len; i++) v = v * 10 + (s->bytes[i] - '0');
            if (k == 0) f.id = v; else if (k == 2) f.qty = v; else f.cents = v;
        }
        ZRec *r = z_rec_new(&f, (ZStr *)parts->e[1]);
        z_list_add(l, r, 0);
        z_dec(r);
        z_dec(parts);                            // frees the array and 3 of its strings
        z_dec(line);
    }
    return l;
}

static Scan z_scan(ZList *l) {
    Scan s = {0, 0};
    ZHdr **e = l->data->e;
    for (int32_t i = 0; i < l->size; i++) {
        ZRec *r = (ZRec *)e[i];
        s.sum += (uint64_t)r->qty * r->cents;
        s.name_bytes += r->name->len;
    }
    return s;
}

static uint64_t z_scan_sorted(ZList *l) {
    uint64_t h = 0xCBF29CE484222325ull;
    ZHdr **e = l->data->e;
    for (int32_t i = 0; i < l->size; i++) {
        ZRec *r = (ZRec *)e[i];
        h = fnv(h, r->name->bytes, r->name->len) ^ r->id;
    }
    return h;
}

static inline int z_less(ZHdr *x, ZHdr *y) {          // a.name.compareTo(b.name)
    ZRec *a = (ZRec *)x, *b = (ZRec *)y;
    ZStr *s = a->name, *t = b->name;
    size_t m = s->len < t->len ? s->len : t->len;
    int c = memcmp(s->bytes, t->bytes, m);
    if (c) return c < 0;
    if (s->len != t->len) return s->len < t->len;
    return a->id < b->id;
}
DEFINE_SORT(z_sort, ZHdr *, z_less)

// The same comparator called through a func value, as a shared list<T>
// instantiation (§13.2) would have to.
static __attribute__((noinline)) int z_less_fn(ZHdr *x, ZHdr *y) { return z_less(x, y); }
static int (*volatile z_less_ptr)(ZHdr *, ZHdr *) = z_less_fn;
#define Z_LESS_INDIRECT(x, y) z_less_ptr((x), (y))
DEFINE_SORT(z_sort_indirect, ZHdr *, Z_LESS_INDIRECT)

// ---------------------------------------------------------------- stacks

static uint8_t *s_recs, *s_recs_top;                  // Rec[..>] with inline names
static uint8_t **s_index, **s_index_top;              // Rec&[..>]
static size_t s_recs_reserved, s_index_reserved;

static void s_regions_new(size_t n) {
    s_recs_reserved = n * 40 + (1 << 20);
    s_index_reserved = n * sizeof(uint8_t *) + (1 << 20);
    s_recs = s_recs_top = reserve(s_recs_reserved);
    s_index = s_index_top = reserve(s_index_reserved);
}
static void s_regions_drop(void) {
    munmap(s_recs, s_recs_reserved);
    munmap(s_index, s_index_reserved);
}

// Record layout: id, qty, cents (4 bytes each), varint name length, name bytes.
static inline uint32_t s_name_len(const uint8_t *r, const uint8_t **bytes) {
    const uint8_t *p = r + 12;
    uint32_t v = 0, shift = 0;
    while (*p & 128) { v |= (uint32_t)(*p++ & 127) << shift; shift += 7; }
    v |= (uint32_t)*p++ << shift;
    *bytes = p;
    return v;
}
static inline uint32_t s_u32(const uint8_t *p) { uint32_t v; memcpy(&v, p, 4); return v; }

//   index.push(recs.push(parse(line)));
static void s_build(void) {
    uint8_t *t = s_recs_top, **ix = s_index_top;
    const char *p = input, *end = input + input_len;
    while (p < end) {
        Fields f;
        p = parse_line(p, &f);
        *ix++ = t;
        memcpy(t, &f.id, 4);
        memcpy(t + 4, &f.qty, 4);
        memcpy(t + 8, &f.cents, 4);
        t += 12;
        uint32_t v = f.len;
        while (v >= 128) { *t++ = (uint8_t)(v | 128); v >>= 7; }
        *t++ = (uint8_t)v;
        memcpy(t, f.name, f.len);
        t += f.len;
    }
    s_recs_top = t;
    s_index_top = ix;
}

static Scan s_scan(void) {                            // for (r in recs)
    Scan s = {0, 0};
    for (const uint8_t *r = s_recs; r < s_recs_top;) {
        const uint8_t *bytes;
        uint32_t len = s_name_len(r, &bytes);
        s.sum += (uint64_t)s_u32(r + 4) * s_u32(r + 8);
        s.name_bytes += len;
        r = bytes + len;
    }
    return s;
}

static uint64_t s_scan_sorted(void) {
    uint64_t h = 0xCBF29CE484222325ull;
    for (uint8_t **ix = s_index; ix < s_index_top; ix++) {
        const uint8_t *bytes;
        uint32_t len = s_name_len(*ix, &bytes);
        h = fnv(h, bytes, len) ^ s_u32(*ix);
    }
    return h;
}

static inline int s_less(const uint8_t *a, const uint8_t *b) {
    const uint8_t *x, *y;
    uint32_t m = s_name_len(a, &x), n = s_name_len(b, &y);
    int c = memcmp(x, y, m < n ? m : n);
    if (c) return c < 0;
    if (m != n) return m < n;
    return s_u32(a) < s_u32(b);
}
DEFINE_SORT(s_sort, uint8_t *, s_less)

static void s_free(void) { s_recs_top = s_recs; s_index_top = s_index; }

// ---------------------------------------------------------------- driver

enum { BUILD, SCAN, SORT, SORTED_SCAN, FREE, PHASES };
static const char *phase_names[PHASES] = {
    "build (parse + store)", "scan in input order", "sort by name",
    "scan in sorted order", "free"};

enum { V_NORMAL, V_SPLIT, V_MOVE, V_INDIRECT };

static size_t n_recs;
static Scan want_scan;
static uint64_t want_sorted;
static double z_bytes_per_rec, s_bytes_per_rec;

// Fresh mode swaps in newly mapped memory for one round and puts the
// long-running heap back afterwards.
typedef struct {
    char *base, *bump; size_t reserved; void *lists[ZSMALL / 8 + 1];
} ZState;
static void z_save(ZState *s) {
    s->base = z_base; s->bump = z_bump; s->reserved = z_reserved;
    memcpy(s->lists, z_free_lists, sizeof z_free_lists);
}
static void z_restore(const ZState *s) {
    z_base = s->base; z_bump = s->bump; z_reserved = s->reserved;
    memcpy(z_free_lists, s->lists, sizeof z_free_lists);
}

static void run_current(int fresh, int variant, double *t) {
    ZState kept;
    if (fresh) { z_save(&kept); z_region_new(n_recs * 200 + (16 << 20)); }
    double t0 = now();
    ZList *l = variant == V_SPLIT ? z_build_split() : z_build(variant == V_MOVE);
    double t1 = now();
    Scan s = z_scan(l);
    double t2 = now();
    if (variant == V_INDIRECT) z_sort_indirect(l->data->e, l->size);
    else z_sort(l->data->e, l->size);
    double t3 = now();
    uint64_t h = z_scan_sorted(l);
    double t4 = now();
    if (fresh && variant == V_NORMAL)
        z_bytes_per_rec = ((double)(z_bump - z_base) + sizeof(ZArr) +
                           l->data->len * sizeof(ZHdr *)) / n_recs;
    z_dec(l);
    double t5 = now();
    check(s.sum == want_scan.sum && s.name_bytes == want_scan.name_bytes, "current scan");
    check(h == want_sorted, "current sorted scan");
    check(z_live == 0, "current leaked");
    t[BUILD] += t1 - t0; t[SCAN] += t2 - t1; t[SORT] += t3 - t2;
    t[SORTED_SCAN] += t4 - t3; t[FREE] += t5 - t4;
    if (fresh) { z_region_drop(); z_restore(&kept); }
}

static void run_stacks(int fresh, double *t) {
    uint8_t *kept_recs = s_recs, **kept_index = s_index;
    if (fresh) s_regions_new(n_recs);
    double t0 = now();
    s_build();
    double t1 = now();
    Scan s = s_scan();
    double t2 = now();
    s_sort(s_index, s_index_top - s_index);
    double t3 = now();
    uint64_t h = s_scan_sorted();
    double t4 = now();
    if (fresh)
        s_bytes_per_rec = ((double)(s_recs_top - s_recs) +
                           (double)(s_index_top - s_index) * sizeof(uint8_t *)) / n_recs;
    s_free();
    double t5 = now();
    if (want_sorted == 0) { want_scan = s; want_sorted = h; }
    check(s.sum == want_scan.sum && s.name_bytes == want_scan.name_bytes, "stacks scan");
    check(h == want_sorted, "stacks sorted scan");
    t[BUILD] += t1 - t0; t[SCAN] += t2 - t1; t[SORT] += t3 - t2;
    t[SORTED_SCAN] += t4 - t3; t[FREE] += t5 - t4;
    if (fresh) {
        s_regions_drop();
        s_recs = s_recs_top = kept_recs;
        s_index = s_index_top = kept_index;
    }
}

static void test(size_t n) {
    n_recs = n;
    rng = 0x9E3779B97F4A7C15ull;
    free(input);
    make_input(n);
    want_sorted = 0;
    int rounds = n >= 1000000 ? 5 : (int)(4000000 / n);
    double cur[2][PHASES] = {{0}}, stk[2][PHASES] = {{0}};
    double var[4][2][PHASES] = {{{0}}};

    // The long-running heap and stacks persist across rounds. One untimed
    // round of each first, then the timed rounds alternate.
    z_region_new(n * 200 + (16 << 20));
    s_regions_new(n);
    size_t s_recs_kept = s_recs_reserved, s_index_kept = s_index_reserved;
    double scratch[PHASES] = {0};
    run_stacks(0, scratch);
    run_current(0, V_NORMAL, scratch);
    for (int r = 0; r < rounds; r++) {
        for (int fresh = 1; fresh >= 0; fresh--) {
            run_stacks(fresh, stk[fresh]);
            run_current(fresh, V_NORMAL, cur[fresh]);
            for (int v = V_SPLIT; v <= V_INDIRECT; v++) run_current(fresh, v, var[v][fresh]);
        }
    }
    z_region_drop();
    s_recs_reserved = s_recs_kept; s_index_reserved = s_index_kept;
    s_regions_drop();

    double scale = 1e9 / ((double)rounds * n);
    printf("\nN = %zu records, ns per record (%d rounds)\n", n, rounds);
    printf("%-34s %31s   %31s\n", "", "fresh memory (one CLI run)", "reused memory (long-running)");
    printf("%-34s %9s %9s %9s   %9s %9s %9s\n", "phase", "current", "stacks", "ratio",
           "current", "stacks", "ratio");
    double tot_c[2] = {0}, tot_s[2] = {0};
    for (int p = 0; p <= PHASES; p++) {
        printf("%-34s", p < PHASES ? phase_names[p] : "total");
        for (int fresh = 1; fresh >= 0; fresh--) {
            double c, s;
            if (p < PHASES) {
                c = cur[fresh][p] * scale; s = stk[fresh][p] * scale;
                tot_c[fresh] += c; tot_s[fresh] += s;
            } else { c = tot_c[fresh]; s = tot_s[fresh]; }
            if (s < 0.05) printf(" %9.1f %9.2f %9s", c, s, "-");
            else printf(" %9.1f %9.1f %8.1fx", c, s, c / s);
            if (fresh) printf("  ");
        }
        printf("\n");
    }
    printf("variants of current, the phase that changes:\n");
    const char *vn[4] = {"", "build, with split() and a line string",
                         "build, if growth moved references", "sort, comparator through a func value"};
    for (int v = V_SPLIT; v <= V_INDIRECT; v++) {
        int p = v == V_INDIRECT ? SORT : BUILD;
        printf("  %-32s", vn[v]);
        for (int fresh = 1; fresh >= 0; fresh--) {
            double c = var[v][fresh][p] * scale, s = stk[fresh][p] * scale;
            printf(" %9.1f %9.1f %8.1fx", c, s, c / s);
            if (fresh) printf("  ");
        }
        printf("\n");
    }
    printf("memory per record: current %.1f bytes, stacks %.1f bytes\n",
           z_bytes_per_rec, s_bytes_per_rec);
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    z_frames_cap = 64;
    z_frames = malloc(z_frames_cap * sizeof *z_frames);
    if (argc > 1) { test((size_t)atol(argv[1])); return 0; }
    test(10000);
    test(1000000);
    printf("\nall results matched between the two models, and nothing leaked\n");
    return 0;
}
