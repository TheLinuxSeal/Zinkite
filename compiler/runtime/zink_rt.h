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
#include <sys/random.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <dirent.h>
#include <sys/uio.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

#define ZR_NORETURN __attribute__((noreturn))
#define ZR_UNUSED __attribute__((unused))

/* i128 and u128 (DESIGN §9.1): GCC's and Clang's 128-bit integers, on 64-bit targets.
 * C has no 128-bit literals, so the limits are built from shifts. */
typedef __int128 zr_i128;
typedef unsigned __int128 zr_u128;
#define ZR_U128_MAX (~(zr_u128)0)
#define ZR_I128_MAX ((zr_i128)(ZR_U128_MAX >> 1))
#define ZR_I128_MIN (-ZR_I128_MAX - 1)

/* A slice or a view: `T[:]`, `T[:]&`, and `string`, which is `u8[:]`. */
typedef struct { void *p; int64_t n; } zr_slice;

/* An array on a data stack: its elements start at p. */
typedef struct { void *p; int64_t n; } zr_vec;

/* An array of variable-size records (DESIGN §3.2): their bytes start at p, one after the
 * other; n records in `bytes` bytes, so the next is built at p + bytes. */
typedef struct { void *p; int64_t n; int64_t bytes; } zr_rvec;

/* Varint lengths (DESIGN §4.6): LEB128, 7 bits a byte, the high bit set on all but the last. */
static ZR_UNUSED int zr_uv_put(uint8_t *p, uint64_t v) {
    int n = 0;
    while (v >= 0x80) { p[n++] = (uint8_t)(v | 0x80); v >>= 7; }
    p[n++] = (uint8_t)v;
    return n;
}

static ZR_UNUSED int zr_uv_get(const uint8_t *p, uint64_t *v) {
    uint64_t r = 0;
    int n = 0, sh = 0;
    uint8_t b;
    do { b = p[n++]; r |= (uint64_t)(b & 0x7F) << sh; sh += 7; } while ((b & 0x80) && sh < 64);
    *v = r;
    return n;
}

/* The bytes of c variable-size values starting at e, each measured by sz. */
static ZR_UNUSED int64_t zr_walk(const uint8_t *e, uint64_t c, int64_t (*sz)(const uint8_t *)) {
    const uint8_t *q = e;
    for (uint64_t i = 0; i < c; i++) q += sz(q);
    return q - e;
}

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

/* Room on the scratch stack until the end of the statement, as a sort's buffer. */
static ZR_UNUSED void *zr_scratch_alloc(size_t sz) {
    zr_sm();
    void *d = zr_scratch_top;
    zr_scratch_top += (sz + 15) & ~(size_t)15;
    return d;
}

/* Text for C, which reads it up to a NUL: copied with one onto the scratch stack, for
 * the call (DESIGN §12.6). */
static ZR_UNUSED const char *zr_cstr(zr_slice s) {
    char *d = (char *)zr_scratch_alloc((size_t)s.n + 1);
    memcpy(d, s.p, (size_t)s.n);
    d[s.n] = 0;
    return d;
}

/* A copy of a temporary on the scratch stack, so a slice of it lasts to the end of its
 * statement. */
static ZR_UNUSED void *zr_scratch_copy(const void *p, size_t sz) {
    zr_sm();
    void *d = zr_scratch_top;
    memcpy(d, p, sz);
    zr_scratch_top += (sz + 15) & ~(size_t)15;
    return d;
}

/* An array that also changes at its start (DESIGN §4.3): its elements are contiguous,
 * starting in the middle of the stack's reservation, so either end can grow. */
typedef struct { void *p; int64_t n; void *mid; } zr_deq;

static ZR_UNUSED zr_deq zr_deq_new(void *base) {
    zr_deq d;
    d.mid = (char *)base + ZR_STACK_BYTES / 2;
    d.p = d.mid;
    d.n = 0;
    return d;
}

/* Back to the middle when empty, so a queue doesn't drift through the reservation. */
static ZR_UNUSED void zr_deq_settle(zr_deq *d) { if (d->n == 0) d->p = d->mid; }

/* The elements of a cursor array (DESIGN §4.5): a data array whose elements never move,
 * so references stay valid. Removals leave holes that the next insert reuses, through a
 * free list inside the holes for elements of at least 4 bytes, and a side list for
 * smaller ones. */
typedef struct {
    char *data;
    int64_t used;    /* slots ever used */
    int64_t freeh;   /* first free hole, plus one; 0 when there is none */
    uint32_t *side;  /* free holes, for elements under 4 bytes */
    int64_t nside;
} zr_data;

static zr_data zr_data_new(void *base) {
    zr_data d;
    memset(&d, 0, sizeof d);
    d.data = (char *)base;
    d.side = (uint32_t *)((char *)base + ZR_STACK_BYTES / 2);
    return d;
}

static ZR_UNUSED uint32_t zr_data_slot(zr_data *d, size_t esz) {
    if (esz >= 4 && d->freeh) {
        uint32_t s = (uint32_t)(d->freeh - 1);
        uint32_t next;
        memcpy(&next, d->data + (size_t)s * esz, 4);
        d->freeh = next;
        return s;
    }
    if (esz < 4 && d->nside) return d->side[--d->nside];
    return (uint32_t)d->used++;
}

static ZR_UNUSED void zr_data_release(zr_data *d, uint32_t s, size_t esz) {
    if (esz >= 4) {
        uint32_t next = (uint32_t)d->freeh;
        memcpy(d->data + (size_t)s * esz, &next, 4);
        d->freeh = (int64_t)s + 1;
    } else {
        d->side[d->nside++] = s;
    }
}

static void zr_data_clear(zr_data *d) { d->used = d->freeh = d->nside = 0; }

/* A cursor array (DESIGN §4.3): the order is an index of 32-bit slots, kept as two
 * deques, the entries before the cursor and those after it, so edits at the cursor and
 * at both ends are O(1), and seek moves 4-byte entries. Two data stacks: the data, and
 * the index. */
typedef struct {
    zr_data d;
    uint32_t *bp, *ap, *bmid, *amid;
    int64_t bn, an;  /* entries before and after the cursor */
} zr_cur;

static ZR_UNUSED zr_cur zr_cur_new(void *data, void *index) {
    zr_cur c;
    memset(&c, 0, sizeof c);
    c.d = zr_data_new(data);
    c.bmid = c.bp = (uint32_t *)((char *)index + ZR_STACK_BYTES / 4);
    c.amid = c.ap = (uint32_t *)((char *)index + ZR_STACK_BYTES / 4 * 3);
    return c;
}

static ZR_UNUSED int64_t zr_cur_len(const zr_cur *c) { return c->bn + c->an; }

/* The slot of the element at logical position i. */
static ZR_UNUSED uint32_t zr_cur_at(const zr_cur *c, int64_t i) { return i < c->bn ? c->bp[i] : c->ap[i - c->bn]; }

static void zr_cur_settle(zr_cur *c) {
    if (c->bn == 0) c->bp = c->bmid;
    if (c->an == 0) c->ap = c->amid;
}

/* Where an insert puts its element: at the cursor, the start or the end. */
static ZR_UNUSED uint32_t zr_cur_insert(zr_cur *c, size_t esz) { uint32_t s = zr_data_slot(&c->d, esz); c->bp[c->bn++] = s; return s; }
static ZR_UNUSED uint32_t zr_cur_push_front(zr_cur *c, size_t esz) { uint32_t s = zr_data_slot(&c->d, esz); *--c->bp = s; c->bn++; return s; }
static ZR_UNUSED uint32_t zr_cur_push(zr_cur *c, size_t esz) { uint32_t s = zr_data_slot(&c->d, esz); c->ap[c->an++] = s; return s; }

/* The slot a removal takes out of the order, or UINT32_MAX when there is none. */
static ZR_UNUSED uint32_t zr_cur_take_before(zr_cur *c) { if (!c->bn) return UINT32_MAX; uint32_t s = c->bp[--c->bn]; zr_cur_settle(c); return s; }
static ZR_UNUSED uint32_t zr_cur_take_after(zr_cur *c) { if (!c->an) return UINT32_MAX; uint32_t s = *c->ap++; c->an--; zr_cur_settle(c); return s; }
static ZR_UNUSED uint32_t zr_cur_take_front(zr_cur *c) {
    uint32_t s;
    if (c->bn) { s = *c->bp++; c->bn--; }
    else if (c->an) { s = *c->ap++; c->an--; }
    else return UINT32_MAX;
    zr_cur_settle(c);
    return s;
}
static ZR_UNUSED uint32_t zr_cur_take_back(zr_cur *c) {
    uint32_t s;
    if (c->an) s = c->ap[--c->an];
    else if (c->bn) s = c->bp[--c->bn];
    else return UINT32_MAX;
    zr_cur_settle(c);
    return s;
}

/* Moves the cursor to position i, from 0 to the length: POSITION otherwise (DESIGN §4.9). */
static ZR_UNUSED uint8_t zr_cur_seek(zr_cur *c, int64_t i) {
    if (i < 0 || i > c->bn + c->an) return 196;
    if (i < c->bn) {
        int64_t k = c->bn - i;
        c->ap -= k;
        memcpy(c->ap, c->bp + i, (size_t)k * 4);
        c->an += k;
        c->bn = i;
    } else if (i > c->bn) {
        int64_t k = i - c->bn;
        memcpy(c->bp + c->bn, c->ap, (size_t)k * 4);
        c->bn += k;
        c->ap += k;
        c->an -= k;
    }
    zr_cur_settle(c);
    return 0;
}

/* A relative move, clamped at the edges. */
static ZR_UNUSED void zr_cur_move(zr_cur *c, int64_t n) {
    int64_t to = c->bn + n, len = c->bn + c->an;
    zr_cur_seek(c, to < 0 ? 0 : to > len ? len : to);
}

static ZR_UNUSED void zr_cur_clear(zr_cur *c) {
    zr_data_clear(&c->d);
    c->bn = c->an = 0;
    c->bp = c->bmid;
    c->ap = c->amid;
}

/* All of the order in the entries before the cursor, contiguous, for reordering; gives the
 * cursor's position, to put back afterwards. */
static ZR_UNUSED int64_t zr_cur_gather(zr_cur *c) { int64_t at = c->bn; zr_cur_seek(c, c->bn + c->an); return at; }

/* A cursor array with a chunked index (DESIGN §4.1, `$`): the same data array, with the
 * index split into chunks of B = 2^L entries, each a circular buffer starting at its
 * head: a two-level tiered vector (Goodrich and Kloss, 1999). An edit anywhere moves at
 * most one chunk's entries and one entry of each later chunk, O(B + n/B); a read is three
 * loads; the cursor is just a position, so seek is free. B doubles when the length passes
 * 4 B^2, keeping it near sqrt(n). Two data stacks: the data, and heads then chunks. */
typedef struct {
    zr_data d;
    uint32_t *heads, *buf;
    int64_t n, pos;
    uint32_t L, M;
} zr_chk;

#define ZR_CHK_HEADS ((size_t)4 << 20)

static ZR_UNUSED zr_chk zr_chk_new(void *data, void *index) {
    zr_chk c;
    memset(&c, 0, sizeof c);
    c.d = zr_data_new(data);
    c.heads = (uint32_t *)index;
    c.buf = (uint32_t *)((char *)index + ZR_CHK_HEADS);
    c.L = 4;
    c.M = 15;
    return c;
}

#define ZR_CHK_SLOT(c, ch, k) ((c)->buf[((size_t)(ch) << (c)->L) + (((c)->heads[ch] + (uint32_t)(k)) & (c)->M)])

static ZR_UNUSED uint32_t zr_chk_at(const zr_chk *c, int64_t i) { return ZR_CHK_SLOT(c, (size_t)i >> c->L, (uint32_t)i & c->M); }

/* The entry at position i, to change in place. */
static ZR_UNUSED uint32_t *zr_chk_entry(zr_chk *c, int64_t i) { return &ZR_CHK_SLOT(c, (size_t)i >> c->L, (uint32_t)i & c->M); }

/* The order, copied in sequence onto the scratch stack, which a statement's end releases. */
static ZR_UNUSED uint32_t *zr_chk_flatten(zr_chk *c) {
    zr_sm();
    uint32_t *f = (uint32_t *)zr_scratch_top;
    for (int64_t i = 0; i < c->n; i++) f[i] = zr_chk_at(c, i);
    zr_scratch_top += (size_t)c->n * 4;
    return f;
}

/* Lays the order out again from `f`, in chunks of 2^L with every head at 0. */
static ZR_UNUSED void zr_chk_unflatten(zr_chk *c, const uint32_t *f) {
    size_t chunks = ((size_t)c->n >> c->L) + 1;
    memset(c->heads, 0, chunks * 4);
    memmove(c->buf, f, (size_t)c->n * 4);
}

static void zr_chk_regrow(zr_chk *c) {
    int64_t m = zr_sm();
    uint32_t *f = zr_chk_flatten(c);
    c->L++;
    c->M = (1u << c->L) - 1;
    zr_chk_unflatten(c, f);
    zr_sr(m);
}

/* Inserts a new element at position i; gives its slot. */
static ZR_UNUSED uint32_t zr_chk_insert_at(zr_chk *c, int64_t i, size_t esz) {
    if (c->n + 1 > (int64_t)4 << (2 * c->L)) zr_chk_regrow(c);
    uint32_t v = zr_data_slot(&c->d, esz), B = c->M + 1, o = (uint32_t)i & c->M;
    size_t ch = (size_t)i >> c->L, last = (size_t)c->n >> c->L;
    if (ch == last) {
        uint32_t m = (uint32_t)c->n & c->M;
        for (uint32_t k = m; k > o; k--) ZR_CHK_SLOT(c, ch, k) = ZR_CHK_SLOT(c, ch, k - 1);
        ZR_CHK_SLOT(c, ch, o) = v;
    } else {
        /* A full chunk: its last entry moves on to the front of the next. */
        uint32_t carry = ZR_CHK_SLOT(c, ch, B - 1);
        for (uint32_t k = B - 1; k > o; k--) ZR_CHK_SLOT(c, ch, k) = ZR_CHK_SLOT(c, ch, k - 1);
        ZR_CHK_SLOT(c, ch, o) = v;
        for (size_t d = ch + 1; d < last; d++) {
            uint32_t h = (c->heads[d] - 1) & c->M;
            uint32_t *p = &c->buf[(d << c->L) + h];
            uint32_t out = *p;
            *p = carry;
            carry = out;
            c->heads[d] = h;
        }
        uint32_t h = (c->heads[last] - 1) & c->M;
        c->buf[(last << c->L) + h] = carry;
        c->heads[last] = h;
    }
    c->n++;
    return v;
}

/* Takes the element at position i out of the order; gives its slot. */
static ZR_UNUSED uint32_t zr_chk_take_at(zr_chk *c, int64_t i) {
    uint32_t B = c->M + 1, o = (uint32_t)i & c->M;
    size_t ch = (size_t)i >> c->L, last = (size_t)(c->n - 1) >> c->L;
    uint32_t v = ZR_CHK_SLOT(c, ch, o);
    if (ch == last) {
        uint32_t m = (uint32_t)(c->n - (int64_t)(last << c->L));
        for (uint32_t k = o; k + 1 < m; k++) ZR_CHK_SLOT(c, ch, k) = ZR_CHK_SLOT(c, ch, k + 1);
    } else {
        for (uint32_t k = o; k + 1 < B; k++) ZR_CHK_SLOT(c, ch, k) = ZR_CHK_SLOT(c, ch, k + 1);
        /* Each later chunk gives its front to the end of the chunk before. */
        for (size_t d = ch + 1; d <= last; d++) {
            uint32_t h = c->heads[d];
            uint32_t front = c->buf[(d << c->L) + h];
            c->heads[d] = (h + 1) & c->M;
            ZR_CHK_SLOT(c, d - 1, B - 1) = front;
        }
    }
    c->n--;
    return v;
}

/* Where an insert goes: 0 the end, 1 the start, 2 the cursor. The cursor stays between
 * the same elements, as for a cursor array. */
static ZR_UNUSED uint32_t zr_chk_put(zr_chk *c, int where, size_t esz) {
    if (where == 0) return zr_chk_insert_at(c, c->n, esz);
    if (where == 1) { c->pos++; return zr_chk_insert_at(c, 0, esz); }
    return zr_chk_insert_at(c, c->pos++, esz);
}

/* What a removal takes: 0 the end, 1 the start, 2 before the cursor, 3 after it.
 * UINT32_MAX when there is nothing. */
static ZR_UNUSED uint32_t zr_chk_take(zr_chk *c, int where) {
    switch (where) {
        case 0: if (!c->n) return UINT32_MAX; if (c->pos == c->n) c->pos--; return zr_chk_take_at(c, c->n - 1);
        case 1: if (!c->n) return UINT32_MAX; if (c->pos > 0) c->pos--; return zr_chk_take_at(c, 0);
        case 2: if (!c->pos) return UINT32_MAX; return zr_chk_take_at(c, --c->pos);
        default: if (c->pos == c->n) return UINT32_MAX; return zr_chk_take_at(c, c->pos);
    }
}

static ZR_UNUSED uint8_t zr_chk_seek(zr_chk *c, int64_t i) {
    if (i < 0 || i > c->n) return 196;
    c->pos = i;
    return 0;
}

static ZR_UNUSED void zr_chk_move(zr_chk *c, int64_t n) {
    int64_t to = c->pos + n;
    c->pos = to < 0 ? 0 : to > c->n ? c->n : to;
}

static ZR_UNUSED void zr_chk_clear(zr_chk *c) {
    zr_data_clear(&c->d);
    c->n = c->pos = 0;
    c->L = 4;
    c->M = 15;
}

/* Capped arrays with a cursor (DESIGN §4.3) keep a small gap buffer inline: gs elements
 * before the gap at the start of e, an after it at the end. The cursor is kept apart, and
 * the gap moves to it only for an edit there, so seeks are free. Moving the gap to
 * position i moves the elements in between across it. */
static ZR_UNUSED void zr_gap_move(char *e, size_t sz, int64_t k, int64_t *gs, int64_t *an, int64_t i) {
    if (i < *gs) {
        int64_t d = *gs - i;
        memmove(e + (size_t)(k - *an - d) * sz, e + (size_t)i * sz, (size_t)d * sz);
        *gs = i;
        *an += d;
    } else if (i > *gs) {
        int64_t d = i - *gs;
        memmove(e + (size_t)*gs * sz, e + (size_t)(k - *an) * sz, (size_t)d * sz);
        *gs += d;
        *an -= d;
    }
}

/* A gap buffer as one block, for a slice (DESIGN §4.10). When the gap splits the
 * elements, all of them, not just the slice's, it moves to whichever end moves fewer.
 * It stays there until the next edit at the cursor, so later slices move nothing and
 * every slice taken since the last edit stays right. The cursor doesn't move. Gives a
 * pointer to position lo. */
static ZR_UNUSED char *zr_gap_span(char *e, size_t sz, int64_t k, int64_t *gs, int64_t *an, int64_t lo) {
    if (*gs && *an) zr_gap_move(e, sz, k, gs, an, *gs <= *an ? 0 : *gs + *an);
    return e + (size_t)(*an ? lo + k - *an : lo) * sz;
}

/* Capped arrays with a start slot are ring buffers (DESIGN §4.3). For sorting, the
 * buffer is rotated in place so its first element, at h, comes to e[0]: three
 * reversals, as k is small. */
static void zr_ring_reverse(char *e, size_t sz, int64_t lo, int64_t hi) {
    for (hi--; lo < hi; lo++, hi--) {
        char *a = e + (size_t)lo * sz, *b = e + (size_t)hi * sz;
        for (size_t i = 0; i < sz; i++) { char t = a[i]; a[i] = b[i]; b[i] = t; }
    }
}

static ZR_UNUSED void zr_ring_line(char *e, size_t sz, int64_t k, int64_t h) {
    if (h == 0) return;
    zr_ring_reverse(e, sz, 0, h);
    zr_ring_reverse(e, sz, h, k);
    zr_ring_reverse(e, sz, 0, k);
}

/* A ring buffer as one block, for a slice: when its elements wrap past the end of e, it
 * is rotated so the first is at e[0], and later slices move nothing until it wraps
 * again. Gives a pointer to position lo. */
static ZR_UNUSED char *zr_ring_span(char *e, size_t sz, int64_t k, int64_t *h, int64_t n, int64_t lo) {
    if (*h + n > k) {
        zr_ring_line(e, sz, k, *h);
        *h = 0;
    }
    return e + (size_t)(*h + lo) * sz;
}

/* ------------------------------------------------------------ output (DESIGN §4.13) */

/* Output goes through C's `stdout`, so what a program prints and what C code it calls
 * prints come out in the order they were written (DESIGN §12.6). stdio buffers it: by
 * lines on a terminal, in 64 KB blocks otherwise (zr_start). stderr is unbuffered, as in
 * C, and stdout is flushed before anything goes to stderr, so the two interleave in order. */
static int zr_out_failed;

#if defined(__GLIBC__)
#define zr_fwrite fwrite_unlocked /* a program is one thread */
#else
#define zr_fwrite fwrite
#endif

/* After a failed write: a reader that has gone ends the program quietly (DESIGN §7.6);
 * anything else is reported at exit (DESIGN §6.4). */
static void zr_check_out(FILE *f) {
    if (ferror(f)) {
        if (errno == EPIPE) exit(141);
        zr_out_failed = 1;
        clearerr(f);
    }
}

static void zr_flush(void) {
    fflush(stdout);
    zr_check_out(stdout);
}

static void zr_out_slow(int fd, const void *p, size_t n) {
    if (fd == 2) {
        fflush(stdout);
        zr_fwrite(p, 1, n, stderr);
        zr_check_out(stderr);
        return;
    }
    if (zr_fwrite(p, 1, n, stdout) < n) zr_check_out(stdout);
}

/* stdout when it's fully buffered (not a terminal), read once: C code writes through
 * the same FILE, so its buffer is shared. NULL on a terminal, where stdio checks every
 * write for a newline. */
static FILE *zr_stdout;

static inline void zr_out(int fd, const void *p, size_t n) {
#if defined(__GLIBC__)
    /* Straight into stdio's buffer while it has room, as glibc's own putc_unlocked does
     * through the same fields. Only when fully buffered: a terminal's line buffering
     * needs stdio to see each newline. */
    FILE *f = zr_stdout;
    if (fd == 1 && f && (size_t)(f->_IO_write_end - f->_IO_write_ptr) >= n) {
        memcpy(f->_IO_write_ptr, p, n);
        f->_IO_write_ptr += n;
        return;
    }
#endif
    zr_out_slow(fd, p, n);
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

/* A terminal sees each line as it ends: stdio line-buffers one. */
static ZR_UNUSED void zr_end_line(int fd) { zr_out(fd, "\n", 1); }

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

/* 128-bit numbers as text: at most 39 digits and a sign. */
static int zr_fmt_u128(char *buf, zr_u128 x) {
    if (x <= UINT64_MAX) return zr_fmt_u64(buf, (uint64_t)x);
    char t[40];
    int n = 0;
    /* Nineteen digits at a time, in 64-bit arithmetic. */
    while (x > UINT64_MAX) {
        uint64_t part = (uint64_t)(x % 10000000000000000000ull);
        x /= 10000000000000000000ull;
        for (int i = 0; i < 19; i++) { t[n++] = (char)('0' + part % 10); part /= 10; }
    }
    for (uint64_t y = (uint64_t)x; y; y /= 10) t[n++] = (char)('0' + y % 10);
    for (int i = 0; i < n; i++) buf[i] = t[n - 1 - i];
    return n;
}
static int zr_fmt_i128(char *buf, zr_i128 x) {
    if (x >= 0) return zr_fmt_u128(buf, (zr_u128)x);
    buf[0] = '-';
    return 1 + zr_fmt_u128(buf + 1, (zr_u128)0 - (zr_u128)x);
}
static ZR_UNUSED void zr_w_i128(zr_sink s, zr_i128 x) { char b[48]; zr_put(s, b, (size_t)zr_fmt_i128(b, x)); }
static ZR_UNUSED void zr_w_u128(zr_sink s, zr_u128 x) { char b[48]; zr_put(s, b, (size_t)zr_fmt_u128(b, x)); }
static ZR_UNUSED void zr_w_i128_pad(zr_sink s, zr_i128 x, int64_t w, char fill) { char b[48]; zr_pad(s, b, zr_fmt_i128(b, x), w, fill); }
static ZR_UNUSED void zr_w_u128_pad(zr_sink s, zr_u128 x, int64_t w, char fill) { char b[48]; zr_pad(s, b, zr_fmt_u128(b, x), w, fill); }
static ZR_UNUSED void zr_w_radix128(zr_sink s, zr_u128 x, int shift) {
    char b[130];
    int n = 0;
    const char *digits = "0123456789abcdef";
    unsigned mask = (1u << shift) - 1;
    do { b[n++] = digits[(unsigned)x & mask]; x >>= shift; } while (x);
    for (int i = n - 1; i >= 0; i--) zr_put(s, &b[i], 1);
}

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
/* A function's array result, kept as a temporary: copied to the scratch stack, where it
 * lasts to the end of the statement, since the callee's next call reuses its data stack. */
static ZR_UNUSED zr_vec zr_vec_scratch(zr_vec v, size_t sz) {
    zr_vec c = {zr_scratch_copy(v.p, (size_t)v.n * sz), v.n};
    return c;
}

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
typedef struct { zr_i128 v; uint8_t s; } zr_res_i128;
typedef struct { zr_u128 v; uint8_t s; } zr_res_u128;

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
static ZR_UNUSED int zr_cmp_su128(zr_i128 a, zr_u128 b) { return a < 0 ? -1 : (zr_u128)a < b ? -1 : (zr_u128)a > b ? 1 : 0; }

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

/* Digits in a base from 2 to 36 into a 128-bit magnitude, up to `limit`: INVALID (22)
 * for a bad digit, RANGE (34) past the limit. */
static uint8_t zr_parse_mag128(const uint8_t *p, int64_t i, int64_t n, int64_t base, zr_u128 limit, zr_u128 *out) {
    if (base < 2 || base > 36 || i == n) return 22;
    zr_u128 acc = 0;
    for (; i < n; i++) {
        int d = p[i] >= '0' && p[i] <= '9' ? p[i] - '0' : p[i] >= 'a' && p[i] <= 'z' ? p[i] - 'a' + 10 : p[i] >= 'A' && p[i] <= 'Z' ? p[i] - 'A' + 10 : 99;
        if (d >= base) return 22;
        if (acc > (limit - (zr_u128)d) / (zr_u128)base) return 34;
        acc = acc * (zr_u128)base + (zr_u128)d;
    }
    *out = acc;
    return 0;
}

static ZR_UNUSED zr_res_u128 zr_parse_u128(zr_slice s, int64_t base) {
    zr_res_u128 r = {0, 22};
    const uint8_t *p = (const uint8_t *)s.p;
    if (s.n == 0) return r;
    zr_u128 v = 0;
    r.s = zr_parse_mag128(p, p[0] == '+' ? 1 : 0, s.n, base, ZR_U128_MAX, &v);
    if (!r.s) r.v = v;
    return r;
}

static ZR_UNUSED zr_res_i128 zr_parse_i128(zr_slice s, int64_t base) {
    zr_res_i128 r = {0, 22};
    const uint8_t *p = (const uint8_t *)s.p;
    if (s.n == 0) return r;
    int neg = p[0] == '-';
    zr_u128 v = 0, limit = neg ? (zr_u128)ZR_I128_MAX + 1 : (zr_u128)ZR_I128_MAX;
    r.s = zr_parse_mag128(p, p[0] == '-' || p[0] == '+' ? 1 : 0, s.n, base, limit, &v);
    if (!r.s) r.v = neg ? (zr_i128)((zr_u128)0 - v) : (zr_i128)v;
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

/* compareNatural (STDLIB §3.3): runs of ASCII digits compare by value, everything else
 * byte by byte, as `ls -v` does; equal numbers written differently fall back to bytes. */
static ZR_UNUSED int32_t zr_cmp_natural(zr_slice a, zr_slice b) {
    const uint8_t *x = (const uint8_t *)a.p, *y = (const uint8_t *)b.p;
    int64_t i = 0, j = 0;
    while (i < a.n && j < b.n) {
        if (x[i] >= '0' && x[i] <= '9' && y[j] >= '0' && y[j] <= '9') {
            int64_t si = i, sj = j;
            while (si < a.n && x[si] == '0') si++;
            while (sj < b.n && y[sj] == '0') sj++;
            int64_t ei = si, ej = sj;
            while (ei < a.n && x[ei] >= '0' && x[ei] <= '9') ei++;
            while (ej < b.n && y[ej] >= '0' && y[ej] <= '9') ej++;
            if (ei - si != ej - sj) return ei - si < ej - sj ? -1 : 1;
            for (int64_t k = 0; k < ei - si; k++)
                if (x[si + k] != y[sj + k]) return x[si + k] < y[sj + k] ? -1 : 1;
            /* The same number: on past it, and decide by bytes only at the end. */
            i = ei;
            j = ej;
            continue;
        }
        if (x[i] != y[j]) return x[i] < y[j] ? -1 : 1;
        i++;
        j++;
    }
    if (i < a.n || j < b.n) return i < a.n ? 1 : -1;
    return zr_cmp(a, b) < 0 ? -1 : zr_cmp(a, b) > 0 ? 1 : 0;
}

/* How long the number at the start of s is, for parsePrefix (STDLIB §2.4): an optional
 * sign and digits, and for a float, a fraction and an exponent. 0 when there's none. */
static ZR_UNUSED int64_t zr_num_prefix(zr_slice s, int32_t is_float) {
    const uint8_t *p = (const uint8_t *)s.p;
    int64_t i = 0, n = s.n, digits = 0;
    if (i < n && (p[i] == '+' || p[i] == '-')) i++;
    while (i < n && p[i] >= '0' && p[i] <= '9') { i++; digits++; }
    if (is_float && i < n && p[i] == '.') {
        int64_t at = i++;
        int64_t frac = 0;
        while (i < n && p[i] >= '0' && p[i] <= '9') { i++; frac++; }
        if (!frac && !digits) i = at;
        digits += frac;
    }
    if (!digits) return 0;
    if (is_float && i < n && (p[i] == 'e' || p[i] == 'E')) {
        int64_t at = i++;
        if (i < n && (p[i] == '+' || p[i] == '-')) i++;
        int64_t ex = 0;
        while (i < n && p[i] >= '0' && p[i] <= '9') { i++; ex++; }
        if (!ex) i = at;
    }
    return i;
}

/* ------------------------------------------------------------ pools (DESIGN §3.5, STDLIB §7.5) */

/* A handle: a slot's index, and the generation the slot had when its value was added. */
typedef struct { uint32_t i, g; } zr_handle;

/* A pool: a grow-only array of slots on a data stack, each `{gen, next, value}`, and its
 * free slots as a list through `next`. A slot's generation is odd while it holds a live
 * value and even once it is freed, so a handle from before a free fails with MISSING.
 * A slot's memory is never released or given another type while the pool lives. */
typedef struct { zr_vec v; uint32_t free; int64_t live; } zr_pool;
#define ZR_NO_SLOT 0xFFFFFFFFu

static ZR_UNUSED zr_pool zr_pool_new(void *base) { zr_pool p = {{base, 0}, ZR_NO_SLOT, 0}; return p; }

static uint32_t *zr_pool_slot(zr_pool *p, uint32_t i, size_t sz) { return (uint32_t *)((char *)p->v.p + (size_t)i * sz); }

/* The slot for a new value: the last one freed, or one more at the end. */
static ZR_UNUSED uint32_t zr_pool_take(zr_pool *p, size_t sz) {
    uint32_t i;
    if (p->free != ZR_NO_SLOT) {
        i = p->free;
        uint32_t *h = zr_pool_slot(p, i, sz);
        p->free = h[1];
        h[0]++;
    } else {
        if (p->v.n >= (int64_t)ZR_NO_SLOT) zr_fail("zink runtime", 0, "a pool holds at most 4294967295 slots");
        i = (uint32_t)p->v.n++;
        zr_pool_slot(p, i, sz)[0] = 1;
    }
    p->live++;
    return i;
}

/* 0 when the handle's value is still there, MISSING once it was freed. A handle to a slot
 * the pool never had is from another pool, or a failed call's zero value: a bug. */
static ZR_UNUSED uint8_t zr_pool_check(zr_pool *p, zr_handle h, size_t sz, const char *f, int l) {
    if ((int64_t)h.i >= p->v.n) zr_fail(f, l, "this handle isn't from this pool");
    return zr_pool_slot(p, h.i, sz)[0] == h.g ? 0 : 195;
}

/* Frees a live slot: its value stays, for any reference still reading it (DESIGN §3.5). */
static ZR_UNUSED void zr_pool_release(zr_pool *p, uint32_t i, size_t sz) {
    uint32_t *h = zr_pool_slot(p, i, sz);
    h[0]++;
    h[1] = p->free;
    p->free = i;
    p->live--;
}

/* Frees every slot, so every handle fails; slot 0 is reused first. */
static ZR_UNUSED void zr_pool_clear(zr_pool *p, size_t sz) {
    p->free = ZR_NO_SLOT;
    for (int64_t i = p->v.n - 1; i >= 0; i--) {
        uint32_t *h = zr_pool_slot(p, (uint32_t)i, sz);
        if (h[0] & 1) h[0]++;
        h[1] = p->free;
        p->free = (uint32_t)i;
    }
    p->live = 0;
}

/* ------------------------------------------------------------ input and files (STDLIB §4) */

/* The status for the errno a failed call left: Zinkite's codes follow errno's (DESIGN §7.7). */
static uint8_t zr_errno(void) { return (uint8_t)(errno > 0 && errno < 192 ? errno : 5); }

/* Per file descriptor: an input buffer, made on its first read and freed when it's closed,
 * and for a File or a Dir, a generation that each close bumps, so a copy of a closed one
 * can't reach whatever reuses its descriptor (DESIGN §6.4). A Dir keeps its DIR here. */
typedef struct { uint8_t *b; size_t pos, len; uint32_t gen; uint8_t open; DIR *dir; } zr_inbuf;
static zr_inbuf *zr_ins;
static int zr_nins;

static zr_inbuf *zr_in_entry(int fd) {
    if (fd >= zr_nins) {
        int n = fd + 16;
        zr_inbuf *t = (zr_inbuf *)realloc(zr_ins, (size_t)n * sizeof *t);
        if (!t) zr_fail("zink runtime", 0, "out of memory for an input buffer");
        memset(t + zr_nins, 0, (size_t)(n - zr_nins) * sizeof *t);
        zr_ins = t;
        zr_nins = n;
    }
    return &zr_ins[fd];
}

static zr_inbuf *zr_in(int fd) {
    zr_inbuf *in = zr_in_entry(fd);
    if (!in->b && !(in->b = (uint8_t *)malloc(1 << 16))) zr_fail("zink runtime", 0, "out of memory for an input buffer");
    return in;
}

/* A File's handle: its generation, then its descriptor. */
static int64_t zr_io_handle(int fd) {
    zr_inbuf *e = zr_in_entry(fd);
    e->open = 1;
    return ((int64_t)e->gen << 32) | (int64_t)fd;
}

/* The descriptor a handle names, or -1 when that File was closed. */
static int zr_io_fd(int64_t h) {
    if (h < 0) return -1;
    int fd = (int)(h & 0xffffffff);
    if (fd >= zr_nins || !zr_ins[fd].open || zr_ins[fd].gen != (uint32_t)(h >> 32)) return -1;
    return fd;
}

/* 0 and a byte, END at the end of input, or the error's status. */
static uint8_t zr_getc(int fd, uint8_t *out) {
    zr_inbuf *in = zr_in(fd);
    if (in->pos == in->len) {
        ssize_t r;
        do { r = read(fd, in->b, 1 << 16); } while (r < 0 && errno == EINTR);
        if (r < 0) return zr_errno();
        if (r == 0) return 194;
        in->pos = 0;
        in->len = (size_t)r;
    }
    *out = in->b[in->pos++];
    return 0;
}

/* Appends everything left on the input to `v`. */
static ZR_UNUSED uint8_t zr_read_all(int fd, zr_vec *v) {
    zr_inbuf *in = zr_in(fd);
    if (in->pos < in->len) {
        memcpy((char *)v->p + v->n, in->b + in->pos, in->len - in->pos);
        v->n += (int64_t)(in->len - in->pos);
        in->pos = in->len;
    }
    for (;;) {
        ssize_t r;
        do { r = read(fd, (char *)v->p + v->n, 1 << 20); } while (r < 0 && errno == EINTR);
        if (r < 0) return zr_errno();
        if (r == 0) return 0;
        v->n += r;
    }
}

/* Appends one line to `v`, without its \n or \r\n: END when nothing was left, though a
 * last line with no newline is kept (STDLIB §4.1). */
static ZR_UNUSED uint8_t zr_read_line(int fd, zr_vec *v) {
    int64_t start = v->n;
    uint8_t b;
    uint8_t s = zr_getc(fd, &b);
    if (s) return s;
    while (b != '\n') {
        ((uint8_t *)v->p)[v->n++] = b;
        s = zr_getc(fd, &b);
        if (s == 194) break;
        if (s) return s;
    }
    if (v->n > start && ((uint8_t *)v->p)[v->n - 1] == '\r') v->n--;
    return 0;
}

/* Clears `v`, reads one line into it, and views it: END when nothing is left. */
static ZR_UNUSED uint8_t zr_next_line(int fd, zr_vec *v, zr_slice *out) {
    v->n = 0;
    uint8_t s = zr_read_line(fd, v);
    out->p = v->p;
    out->n = s ? 0 : v->n;
    return s;
}

/* zink.io's C layer: File (STDLIB §4.1). Paths come as slices and are copied with a
 * NUL onto the scratch stack, released before returning. */

/* Opens for File.open (0), create (1), append (2), createNew (3) or update (4): the
 * File's handle, or -1 with errno set. */
static ZR_UNUSED int64_t zr_io_open(zr_slice path, int32_t how) {
    static const int flags[] = {
        O_RDONLY, O_WRONLY | O_CREAT | O_TRUNC, O_WRONLY | O_CREAT | O_APPEND,
        O_WRONLY | O_CREAT | O_EXCL, O_RDWR,
    };
    if (how < 0 || how > 4) { errno = EINVAL; return -1; }
    int64_t m = zr_sm();
    const char *p = zr_cstr(path);
    int fd;
    do { fd = open(p, flags[how] | O_CLOEXEC, 0666); } while (fd < 0 && errno == EINTR);
    zr_sr(m);
    return fd < 0 ? -1 : zr_io_handle(fd);
}

/* BAD_HANDLE (EBADF) for a File already closed. */
static ZR_UNUSED uint8_t zr_io_close(int64_t h) {
    int fd = zr_io_fd(h);
    if (fd < 0) return 9;
    if (fd == 1) zr_flush();
    zr_inbuf *e = &zr_ins[fd];
    free(e->b);
    e->b = NULL;
    e->pos = e->len = 0;
    e->open = 0;
    e->gen++;
    return close(fd) < 0 && errno != EINTR ? zr_errno() : 0;
}

static uint8_t zr_write_fd(int fd, zr_slice s);

/* Writes all of `s`: 0, or the error's status. */
/* stdout and stderr through a File go where print does, so they stay in order with it
 * (STDLIB §2.1). A failed write to stdout is reported at exit, and at the next flush. */
static uint8_t zr_std_write(int fd, zr_slice s) {
    zr_out(fd, s.p, (size_t)s.n);
    return 0;
}

static ZR_UNUSED uint8_t zr_io_write(int64_t h, zr_slice s) {
    int fd = zr_io_fd(h);
    if (fd == 1 || fd == 2) return zr_std_write(fd, s);
    return fd < 0 ? 9 : zr_write_fd(fd, s);
}

/* Writes what stdout holds; nothing else is buffered. */
static ZR_UNUSED uint8_t zr_io_flush(int64_t h) {
    int fd = zr_io_fd(h);
    if (fd < 0) return 9;
    if (fd != 1) return 0;
    if (fflush(stdout) == 0) return 0;
    uint8_t s = zr_errno();
    if (errno == EPIPE) exit(141);
    clearerr(stdout);
    return s;
}

/* File.println: the text and its newline in one writev, then whatever a short write left. */
static ZR_UNUSED uint8_t zr_io_write_line(int64_t h, zr_slice s) {
    int fd = zr_io_fd(h);
    if (fd < 0) return 9;
    if (fd == 1 || fd == 2) {
        zr_out(fd, s.p, (size_t)s.n);
        zr_end_line(fd);
        return 0;
    }
    struct iovec v[2] = {{(void *)s.p, (size_t)s.n}, {(void *)"\n", 1}};
    ssize_t r;
    do { r = writev(fd, v, 2); } while (r < 0 && errno == EINTR);
    if (r < 0) return zr_errno();
    if (r == s.n + 1) return 0;
    if (r < s.n) {
        zr_slice rest = {(uint8_t *)s.p + r, s.n - r};
        uint8_t e = zr_write_fd(fd, rest);
        if (e) return e;
    }
    zr_slice nl = {(void *)"\n", 1};
    return zr_write_fd(fd, nl);
}

static uint8_t zr_write_fd(int fd, zr_slice s) {
    const char *p = (const char *)s.p;
    size_t n = (size_t)s.n;
    while (n > 0) {
        ssize_t w = write(fd, p, n);
        if (w < 0) {
            if (errno == EINTR) continue;
            return zr_errno();
        }
        p += w;
        n -= (size_t)w;
    }
    return 0;
}

/* Appends up to `max` bytes to `v`: how many, 0 at the end, or -1 with errno set. */
static ZR_UNUSED int64_t zr_io_read(int64_t h, zr_vec *v, int64_t max) {
    int fd = zr_io_fd(h);
    if (fd < 0) { errno = EBADF; return -1; }
    if (max <= 0) return 0;
    zr_inbuf *in = zr_in(fd);
    if (in->pos < in->len) {
        size_t k = in->len - in->pos;
        if ((int64_t)k > max) k = (size_t)max;
        memcpy((char *)v->p + v->n, in->b + in->pos, k);
        in->pos += k;
        v->n += (int64_t)k;
        return (int64_t)k;
    }
    ssize_t r;
    do { r = read(fd, (char *)v->p + v->n, (size_t)max); } while (r < 0 && errno == EINTR);
    if (r < 0) return -1;
    v->n += r;
    return r;
}

/* For File.readLine and File.readAll. */
static ZR_UNUSED uint8_t zr_io_read_line(int64_t h, zr_vec *v) {
    int fd = zr_io_fd(h);
    return fd < 0 ? 9 : zr_read_line(fd, v);
}

static ZR_UNUSED uint8_t zr_io_read_all(int64_t h, zr_vec *v) {
    int fd = zr_io_fd(h);
    return fd < 0 ? 9 : zr_read_all(fd, v);
}

/* About a File: 0 its size, 1 whether it's a terminal, 2 its position; -1 with errno set. */
static ZR_UNUSED int64_t zr_io_info(int64_t h, int32_t what) {
    int fd = zr_io_fd(h);
    if (fd < 0) { errno = EBADF; return -1; }
    struct stat st;
    switch (what) {
        case 0: return fstat(fd, &st) < 0 ? -1 : (int64_t)st.st_size;
        case 1: return isatty(fd);
        default: {
            off_t at = lseek(fd, 0, SEEK_CUR);
            if (at < 0) return -1;
            /* Bytes read ahead into the buffer haven't been read by the program yet. */
            if (fd < zr_nins) at -= (off_t)(zr_ins[fd].len - zr_ins[fd].pos);
            return (int64_t)at;
        }
    }
}

static ZR_UNUSED uint8_t zr_io_seek(int64_t h, int64_t pos) {
    int fd = zr_io_fd(h);
    if (fd < 0) return 9;
    zr_ins[fd].pos = zr_ins[fd].len = 0;
    return lseek(fd, (off_t)pos, SEEK_SET) < 0 ? zr_errno() : 0;
}

static ZR_UNUSED uint8_t zr_io_sync(int64_t h) {
    int fd = zr_io_fd(h);
    if (fd < 0) return 9;
    return fsync(fd) < 0 ? zr_errno() : 0;
}

/* File.readAll(path, buf): open, read to the end, close. */
static ZR_UNUSED uint8_t zr_io_read_path(zr_slice path, zr_vec *v) {
    int64_t h = zr_io_open(path, 0);
    if (h < 0) return zr_errno();
    uint8_t s = zr_io_read_all(h, v);
    uint8_t c = zr_io_close(h);
    return s ? s : c;
}

/* File.writeAll: a temporary file next to the target, renamed over it, so another process
 * sees the old contents or the new, never half. A symbolic link is followed, and the old
 * file's permissions are kept. File.appendAll (append = 1) adds to the end instead. */
static ZR_UNUSED uint8_t zr_io_write_path(zr_slice path, zr_slice data, int32_t append) {
    if (append) {
        int64_t h = zr_io_open(path, 2);
        if (h < 0) return zr_errno();
        uint8_t s = zr_io_write(h, data);
        uint8_t c = zr_io_close(h);
        return s ? s : c;
    }
    int64_t m = zr_sm();
    const char *p = zr_cstr(path);
    char *real = realpath(p, NULL);
    const char *target = real ? real : p;
    struct stat st;
    int existed = stat(target, &st) == 0;
    static uint64_t serial;
    size_t tn = strlen(target) + 64;
    char *tmp = (char *)zr_scratch_alloc(tn);
    snprintf(tmp, tn, "%s.zink-%ld-%llu.tmp", target, (long)getpid(), (unsigned long long)++serial);
    uint8_t s = 0;
    int fd;
    do { fd = open(tmp, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0666); } while (fd < 0 && errno == EINTR);
    if (fd < 0) s = zr_errno();
    if (!s) s = zr_write_fd(fd, data);
    if (!s && existed && fchmod(fd, st.st_mode & 07777) < 0) s = zr_errno();
    if (fd >= 0 && close(fd) < 0 && !s) s = zr_errno();
    if (!s && rename(tmp, target) < 0) s = zr_errno();
    if (s && fd >= 0) unlink(tmp);
    free(real);
    zr_sr(m);
    return s;
}

/* zink.io's C layer: fs (STDLIB §4.4). */

/* What stat or lstat says, into out: size, modified (seconds, nanoseconds), mode, uid,
 * gid, and the kind: 1 a directory, 2 a file, 3 a link, 0 anything else. */
static ZR_UNUSED uint8_t zr_fs_stat(zr_slice path, int32_t follow, zr_slice out) {
    int64_t m = zr_sm();
    struct stat st;
    int r = follow ? stat(zr_cstr(path), &st) : lstat(zr_cstr(path), &st);
    zr_sr(m);
    if (r < 0) return zr_errno();
    int64_t *o = (int64_t *)out.p;
    if (out.n < 7) return 22;
    o[0] = (int64_t)st.st_size;
    o[1] = (int64_t)st.st_mtim.tv_sec;
    o[2] = (int64_t)st.st_mtim.tv_nsec;
    o[3] = (int64_t)(st.st_mode & 07777);
    o[4] = (int64_t)st.st_uid;
    o[5] = (int64_t)st.st_gid;
    o[6] = S_ISDIR(st.st_mode) ? 1 : S_ISREG(st.st_mode) ? 2 : S_ISLNK(st.st_mode) ? 3 : 0;
    return 0;
}

/* One file system call on one or two paths: 0 remove, 1 rename, 2 mkdir, 3 chmod,
 * 4 symlink (a is the target), 5 exists. 0, or the error's status; for exists, 0 or 1. */
static ZR_UNUSED uint8_t zr_fs_op(int32_t op, zr_slice a, zr_slice b, int64_t mode) {
    int64_t m = zr_sm();
    const char *pa = zr_cstr(a);
    const char *pb = op == 1 || op == 4 ? zr_cstr(b) : NULL;
    int r;
    switch (op) {
        case 0: r = remove(pa); break;
        case 1: r = rename(pa, pb); break;
        case 2: r = mkdir(pa, (mode_t)mode); break;
        case 3: r = chmod(pa, (mode_t)mode); break;
        case 4: r = symlink(pa, pb); break;
        default: {
            struct stat st;
            r = stat(pa, &st) == 0;
            zr_sr(m);
            return (uint8_t)r;
        }
    }
    uint8_t s = r < 0 ? zr_errno() : 0;
    zr_sr(m);
    return s;
}

/* Appends a link's target, or the working directory (`what` 1), to v. */
static ZR_UNUSED uint8_t zr_fs_read_into(zr_slice path, zr_vec *v, int32_t what) {
    int64_t m = zr_sm();
    char *dst = (char *)v->p + v->n;
    ssize_t r;
    if (what == 1) {
        r = getcwd(dst, 1 << 20) ? (ssize_t)strlen(dst) : -1;
    } else if (what == 2) {
        char *real = realpath(zr_cstr(path), NULL);
        r = -1;
        if (real) {
            r = (ssize_t)strlen(real);
            memcpy(dst, real, (size_t)r);
            free(real);
        }
    } else {
        r = readlink(zr_cstr(path), dst, 1 << 20);
    }
    zr_sr(m);
    if (r < 0) return zr_errno();
    v->n += r;
    return 0;
}

/* fs.copy: the contents and the mode, with copy_file_range where the kernel can, and read
 * and write where it can't. IS_DIRECTORY for a directory. */
static ZR_UNUSED uint8_t zr_fs_copy(zr_slice from, zr_slice to) {
    int64_t m = zr_sm();
    const char *pf = zr_cstr(from), *pt = zr_cstr(to);
    uint8_t s = 0;
    int in, out = -1;
    struct stat st;
    do { in = open(pf, O_RDONLY | O_CLOEXEC); } while (in < 0 && errno == EINTR);
    if (in < 0 || fstat(in, &st) < 0) s = zr_errno();
    else if (S_ISDIR(st.st_mode)) s = 21;
    if (!s) {
        do { out = open(pt, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, st.st_mode & 07777); } while (out < 0 && errno == EINTR);
        if (out < 0) s = zr_errno();
    }
    int plain = 0;
#ifndef SYS_copy_file_range
    plain = 1;
#endif
    while (!s) {
        ssize_t r;
        if (!plain) {
#ifdef SYS_copy_file_range
            r = (ssize_t)syscall(SYS_copy_file_range, in, NULL, out, NULL, (size_t)1 << 30, 0u);
#else
            r = -1;
#endif
            /* Not across these file systems, or not this kernel: copy it by hand. The offsets
             * moved with what was copied, so read and write go on from there. */
            if (r < 0 && (errno == EXDEV || errno == ENOSYS || errno == EINVAL || errno == EOPNOTSUPP)) {
                plain = 1;
                continue;
            }
        } else {
            static char buf[1 << 16];
            r = read(in, buf, sizeof buf);
            if (r > 0) {
                zr_slice piece = {buf, r};
                s = zr_write_fd(out, piece);
            }
        }
        if (r < 0 && errno == EINTR) continue;
        if (r < 0) s = zr_errno();
        if (r <= 0) break;
    }
    if (!s && fchmod(out, st.st_mode & 07777) < 0) s = zr_errno();
    if (out >= 0 && close(out) < 0 && !s) s = zr_errno();
    if (in >= 0) close(in);
    zr_sr(m);
    return s;
}

/* fs.tempFile and fs.tempDir: a new name in $TMPDIR, or /tmp, made of random hex from
 * getrandom and created exclusively, appended to `path`. A File's handle (mode 0600), or 0
 * for a directory (mode 0700); -1 and errno when it can't be made. */
static ZR_UNUSED int64_t zr_fs_temp(zr_vec *path, int32_t dir) {
    const char *base = getenv("TMPDIR");
    if (!base || !*base) base = "/tmp";
    size_t bn = strlen(base);
    while (bn > 1 && base[bn - 1] == '/') bn--;
    if (bn > 4000) {
        errno = ENAMETOOLONG;
        return -1;
    }
    char name[4096];
    for (int tries = 0; tries < 100; tries++) {
        uint8_t r[8];
        if (getrandom(r, sizeof r, 0) != (ssize_t)sizeof r) {
            if (errno == EINTR) continue;
            return -1;
        }
        size_t n = bn;
        memcpy(name, base, bn);
        if (!(bn == 1 && base[0] == '/')) name[n++] = '/';
        memcpy(name + n, "zink-", 5);
        n += 5;
        for (int i = 0; i < 8; i++) {
            name[n++] = "0123456789abcdef"[r[i] >> 4];
            name[n++] = "0123456789abcdef"[r[i] & 15];
        }
        name[n] = 0;
        int64_t h = 0;
        if (dir) {
            if (mkdir(name, 0700) < 0) {
                if (errno == EEXIST) continue;
                return -1;
            }
        } else {
            int fd;
            do { fd = open(name, O_RDWR | O_CREAT | O_EXCL | O_CLOEXEC, 0600); } while (fd < 0 && errno == EINTR);
            if (fd < 0) {
                if (errno == EEXIST) continue;
                return -1;
            }
            h = zr_io_handle(fd);
        }
        memcpy((char *)path->p + path->n, name, n);
        path->n += (int64_t)n;
        return h;
    }
    errno = EEXIST;
    return -1;
}

/* zink.io's C layer: Dir (STDLIB §4.2). Its handle names the directory's descriptor, with
 * a generation, as a File's does. -1 and errno when it can't be opened. */
static ZR_UNUSED int64_t zr_dir_open(zr_slice path) {
    int64_t m = zr_sm();
    DIR *d = opendir(zr_cstr(path));
    int e = errno;
    zr_sr(m);
    if (!d) {
        errno = e;
        return -1;
    }
    int fd = dirfd(d);
    zr_in_entry(fd)->dir = d;
    return zr_io_handle(fd);
}

/* The next entry but . and ..: its name appended to `names`, and in out[0..3) whether it's
 * a directory, whether it's a link, and its size, from one fstatat. END after the last. */
static ZR_UNUSED uint8_t zr_dir_next(int64_t h, zr_vec *names, zr_slice out) {
    int fd = zr_io_fd(h);
    if (fd < 0 || !zr_ins[fd].dir) return 9;
    DIR *d = zr_ins[fd].dir;
    struct dirent *e;
    do {
        errno = 0;
        e = readdir(d);
        if (!e) return errno ? zr_errno() : 194;
    } while (e->d_name[0] == '.' && (!e->d_name[1] || (e->d_name[1] == '.' && !e->d_name[2])));
    size_t len = strlen(e->d_name);
    memcpy((char *)names->p + names->n, e->d_name, len);
    names->n += (int64_t)len;
    struct stat st;
    int have = fstatat(fd, e->d_name, &st, AT_SYMLINK_NOFOLLOW) == 0;
    int kind = e->d_type;
    if (kind == DT_UNKNOWN && have)
        kind = S_ISDIR(st.st_mode) ? DT_DIR : S_ISLNK(st.st_mode) ? DT_LNK : DT_REG;
    int64_t *o = (int64_t *)out.p;
    o[0] = kind == DT_DIR;
    o[1] = kind == DT_LNK;
    o[2] = have ? (int64_t)st.st_size : 0;
    return 0;
}

/* BAD_HANDLE when it was already closed. */
static ZR_UNUSED uint8_t zr_dir_close(int64_t h) {
    int fd = zr_io_fd(h);
    if (fd < 0 || !zr_ins[fd].dir) return 9;
    zr_inbuf *e = &zr_ins[fd];
    DIR *d = e->dir;
    e->dir = NULL;
    e->open = 0;
    e->gen++;
    return closedir(d) < 0 ? zr_errno() : 0;
}

static ZR_UNUSED uint8_t zr_fs_chdir(zr_slice path) {
    int64_t m = zr_sm();
    int r = chdir(zr_cstr(path));
    zr_sr(m);
    return r < 0 ? zr_errno() : 0;
}

/* ------------------------------------------------------------ the program's start */

static zr_vec zr_args;

/* ------------------------------------------------------------ hash maps (DESIGN §4.14) */

/* A random seed per run, so inputs can't be crafted to collide; iteration follows
 * insertion order, so output is the same on every run. */
static uint64_t zr_seed;

static void zr_seed_init(void) {
    if (getrandom(&zr_seed, sizeof zr_seed, GRND_NONBLOCK) != (ssize_t)sizeof zr_seed)
        zr_seed = (uint64_t)time(NULL) ^ ((uint64_t)getpid() << 32) ^ (uint64_t)(uintptr_t)&zr_seed;
    zr_seed |= 1;
}

/* Multiply into 128 bits and fold: the mixing step of wyhash. */
static inline uint64_t zr_mum(uint64_t a, uint64_t b) {
    zr_u128 p = (zr_u128)a * b;
    return (uint64_t)p ^ (uint64_t)(p >> 64);
}

static ZR_UNUSED inline uint64_t zr_hash_u64(uint64_t x) {
    return zr_mum(x ^ zr_seed, 0x9e3779b97f4a7c15ull);
}

static ZR_UNUSED inline uint64_t zr_hash_u128(zr_u128 x) {
    return zr_mum(zr_hash_u64((uint64_t)x) ^ (uint64_t)(x >> 64), 0xa0761d6478bd642full);
}

static ZR_UNUSED uint64_t zr_hash_bytes(const void *p, int64_t n) {
    const uint8_t *b = (const uint8_t *)p;
    uint64_t h = zr_seed ^ zr_mum((uint64_t)n, 0xe7037ed1a0b428dbull);
    while (n >= 8) {
        uint64_t w;
        memcpy(&w, b, 8);
        h = zr_mum(h ^ w, 0xa0761d6478bd642full);
        b += 8;
        n -= 8;
    }
    uint64_t t = 0;
    memcpy(&t, b, (size_t)n);
    return zr_mum(h ^ t, 0x8ebc6af09c88c6e3ull);
}

/* A hashmap or hashset: Python's compact dict on one data stack. Entries are appended
 * in insertion order from the stack's base and never move on insert; removing marks
 * one dead. The index, at half the reservation, holds entry numbers plus one (0 empty,
 * all ones removed) in 1, 2, 4 or 8 bytes, as few as the capacity allows, and is
 * rebuilt when the entries outgrow it. Each map's entry type and operations are
 * generated with the program. */
typedef struct {
    char *e;
    int64_t n;     /* entries used, dead ones included */
    int64_t live;  /* the map's length */
    int64_t cap;   /* entries before the index is rebuilt */
    char *ix;
    int64_t mask;  /* index slots, less one */
    int w;         /* bytes per index slot */
} zr_map;

static ZR_UNUSED zr_map zr_map_new(void *base) {
    zr_map m;
    memset(&m, 0, sizeof m);
    m.e = (char *)base;
    m.ix = (char *)base + ZR_STACK_BYTES / 2;
    return m;
}

static inline uint64_t zr_ix_dead(const zr_map *m) { return m->w == 8 ? UINT64_MAX : ((uint64_t)1 << (8 * m->w)) - 1; }

static inline uint64_t zr_ix_get(const zr_map *m, int64_t i) {
    switch (m->w) {
        case 1: return ((uint8_t *)m->ix)[i];
        case 2: return ((uint16_t *)m->ix)[i];
        case 4: return ((uint32_t *)m->ix)[i];
        default: return ((uint64_t *)m->ix)[i];
    }
}

static inline void zr_ix_set(zr_map *m, int64_t i, uint64_t v) {
    switch (m->w) {
        case 1: ((uint8_t *)m->ix)[i] = (uint8_t)v; break;
        case 2: ((uint16_t *)m->ix)[i] = (uint16_t)v; break;
        case 4: ((uint32_t *)m->ix)[i] = (uint32_t)v; break;
        default: ((uint64_t *)m->ix)[i] = v; break;
    }
}

/* Sizes an empty index for `cap` entries, at most two thirds full. */
static ZR_UNUSED void zr_map_size(zr_map *m, int64_t cap) {
    if (cap < 8) cap = 8;
    m->cap = cap;
    m->w = cap < 250 ? 1 : cap < 65530 ? 2 : cap < 4294967290ll ? 4 : 8;
    int64_t slots = 16;
    while (slots < cap + cap / 2 + 1) slots *= 2;
    m->mask = slots - 1;
    memset(m->ix, 0, (size_t)slots * (size_t)m->w);
}

static ZR_UNUSED void zr_map_clear(zr_map *m) {
    m->n = m->live = 0;
    if (m->cap) memset(m->ix, 0, (size_t)(m->mask + 1) * (size_t)m->w);
}

static ZR_UNUSED void zr_start(int argc, char **argv) {
    signal(SIGPIPE, SIG_IGN);
    /* glibc ignores the size unless it's given the buffer too. */
    static char obuf[1 << 16];
    if (!isatty(1)) {
        setvbuf(stdout, obuf, _IOFBF, sizeof obuf);
        zr_stdout = stdout;
    }
    zr_seed_init();
    /* stdin, stdout and stderr are Files from the start (STDLIB §2.1). */
    for (int fd = 0; fd < 3; fd++) zr_in_entry(fd)->open = 1;
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
