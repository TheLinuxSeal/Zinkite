/* The Zinkite runtime (DESIGN §12.2), included at the top of every generated C file.
 * Data stacks, output, failures, checked arithmetic, and the core of the library. */

#include <errno.h>
#include <setjmp.h>
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
#include <spawn.h>
#include <poll.h>
#include <sys/wait.h>
#include <sys/ioctl.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

#define ZR_NORETURN __attribute__((noreturn))
#define ZR_UNUSED __attribute__((unused))

/* Threads (STDLIB §10.3). A program that imports zink.thread is compiled with ZR_THREADS:
 * each thread then has data stacks, a scratch stack, a failure trail, `@@` records and
 * open Files of its own, as if it were a program of its own, and output and the
 * environment are locked. A program that doesn't is compiled as before. */
/* ThreadSanitizer doesn't model flockfile, which guards stdout's buffer; built with it,
 * output goes through fwrite, which it does model, so it reports no false races. */
#if defined(__SANITIZE_THREAD__)
#define ZR_TSAN 1
#elif defined(__has_feature)
#if __has_feature(thread_sanitizer)
#define ZR_TSAN 1
#endif
#endif

#ifdef ZR_THREADS
#include <pthread.h>
#define ZR_TLS __thread
#define ZR_OUT_LOCK() flockfile(stdout)
#define ZR_OUT_UNLOCK() funlockfile(stdout)
#else
#define ZR_TLS
#define ZR_OUT_LOCK() ((void)0)
#define ZR_OUT_UNLOCK() ((void)0)
#endif

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

static __attribute__((noinline)) int zr_uv_get_long(const uint8_t *p, uint64_t *v) {
    uint64_t r = 0;
    int n = 0, sh = 0;
    uint8_t b;
    do { b = p[n++]; r |= (uint64_t)(b & 0x7F) << sh; sh += 7; } while ((b & 0x80) && sh < 64);
    *v = r;
    return n;
}

/* Most lengths are under 128, one byte: that case is a load and a test where the length is
 * read, as when stepping over each record of an array, and the rest is a call. */
static ZR_UNUSED inline int zr_uv_get(const uint8_t *p, uint64_t *v) {
    if (!(p[0] & 0x80)) { *v = p[0]; return 1; }
    return zr_uv_get_long(p, v);
}

/* The bytes of c variable-size values starting at e, each measured by sz. */
static ZR_UNUSED int64_t zr_walk(const uint8_t *e, uint64_t c, int64_t (*sz)(const uint8_t *)) {
    const uint8_t *q = e;
    for (uint64_t i = 0; i < c; i++) q += sz(q);
    return q - e;
}

/* ------------------------------------------------------------ short copies and compares */

/* Keys, words and names are mostly 16 bytes or fewer. For those, a call to the C library's
 * memcpy or memcmp, and its dispatch on the length, costs more than the work: these do it
 * with two overlapping loads a side, and call the library only for longer runs. */
static inline uint64_t zr_ld64(const void *p) { uint64_t v; memcpy(&v, p, 8); return v; }
static inline uint32_t zr_ld32(const void *p) { uint32_t v; memcpy(&v, p, 4); return v; }

/* Up to 256 bytes, 16 at a time with the last 16 overlapping; the library beyond. Out of
 * line, so that zr_memeq stays small enough to inline everywhere. */
static __attribute__((noinline)) bool zr_memeq_long(const uint8_t *p, const uint8_t *q, size_t n) {
    if (n > 256) return memcmp(p, q, n) == 0;
    for (size_t i = 0; i + 16 < n; i += 16)
        if ((zr_ld64(p + i) ^ zr_ld64(q + i)) | (zr_ld64(p + i + 8) ^ zr_ld64(q + i + 8))) return false;
    return ((zr_ld64(p + n - 16) ^ zr_ld64(q + n - 16)) | (zr_ld64(p + n - 8) ^ zr_ld64(q + n - 8))) == 0;
}

/* Whether n bytes at a and b are equal. Either may be NULL when n is 0. */
static inline bool zr_memeq(const void *a, const void *b, size_t n) {
    const uint8_t *p = (const uint8_t *)a, *q = (const uint8_t *)b;
    if (n >= 8 && n <= 16)
        return ((zr_ld64(p) ^ zr_ld64(q)) | (zr_ld64(p + n - 8) ^ zr_ld64(q + n - 8))) == 0;
    if (n >= 4 && n < 8)
        return ((zr_ld32(p) ^ zr_ld32(q)) | (zr_ld32(p + n - 4) ^ zr_ld32(q + n - 4))) == 0;
    if (n < 4)
        return n == 0 || ((p[0] ^ q[0]) | (p[n >> 1] ^ q[n >> 1]) | (p[n - 1] ^ q[n - 1])) == 0;
    return zr_memeq_long(p, q, n);
}

/* memcpy: runs that don't overlap. Up to 16 bytes, both ends are loaded before either is
 * stored. Either pointer may be NULL when n is 0. */
static inline void zr_copy(void *dst, const void *src, size_t n) {
    uint8_t *d = (uint8_t *)dst;
    const uint8_t *s = (const uint8_t *)src;
    if (n > 16) {
        memcpy(d, s, n);
    } else if (n >= 8) {
        uint64_t a = zr_ld64(s), b = zr_ld64(s + n - 8);
        memcpy(d, &a, 8);
        memcpy(d + n - 8, &b, 8);
    } else if (n >= 4) {
        uint32_t a = zr_ld32(s), b = zr_ld32(s + n - 4);
        memcpy(d, &a, 4);
        memcpy(d + n - 4, &b, 4);
    } else if (n) {
        uint8_t a = s[0], b = s[n >> 1], c = s[n - 1];
        d[0] = a;
        d[n >> 1] = b;
        d[n - 1] = c;
    }
}

/* ------------------------------------------------------------ failures (DESIGN §7.6) */

static void zr_flush(void);

/* The runtime's own failures, such as running out of address space: nothing catches them. */
static ZR_NORETURN ZR_UNUSED void zr_fail(const char *file, int line, const char *msg) {
    zr_flush();
    fprintf(stderr, "%s:%d: %s\n", file, line, msg);
    exit(1);
}

/* A line that can fail, and the function it's in. The library's have no function, add
 * nothing to the trail, and live only while they're reported. */
typedef struct { const char *fn; const char *file; int line; } zr_site;

static const char *zr_status_name(uint8_t s);

/* ---- the trail: where the latest failure was returned, passed on, and left unread. A
 * longer one keeps its first 24 entries and its last 8. */
typedef struct { const zr_site *site; uint8_t code; uint8_t unread; } zr_trail_entry;
#define ZR_TRAIL 32
#define ZR_TRAIL_HEAD 24
static ZR_TLS zr_trail_entry zr_trail[ZR_TRAIL];
static ZR_TLS int64_t zr_trail_n;          /* entries added, counting those dropped */
static ZR_TLS uint64_t zr_trail_gen;       /* which trail it is */
static ZR_TLS int zr_trail_live;           /* the latest failure added to it, and nothing has read it */

static void zr_trail_add(const zr_site *s, uint8_t code, int unread, int fresh) {
    if (fresh) {
        zr_trail_gen++;
        zr_trail_n = 0;
    }
    int64_t i = zr_trail_n < ZR_TRAIL ? zr_trail_n
        : ZR_TRAIL_HEAD + (zr_trail_n - ZR_TRAIL_HEAD) % (ZR_TRAIL - ZR_TRAIL_HEAD);
    zr_trail[i] = (zr_trail_entry){s, code, (uint8_t)unread};
    zr_trail_n++;
    zr_trail_live = 1;
}

/* Entry m of the trail, counting from its start, if it's kept: all of the first 24, and
 * the last 8 of a longer one. */
static zr_trail_entry *zr_trail_ord(int64_t m) {
    return &zr_trail[m < ZR_TRAIL ? m : ZR_TRAIL_HEAD + (m - ZR_TRAIL_HEAD) % (ZR_TRAIL - ZR_TRAIL_HEAD)];
}

/* A function of the program's returns a failure. It adds to the trail of the failure it
 * read last when `g` says that one is still the latest, and starts a new trail otherwise. */
static ZR_UNUSED void zr_trail_ret(const zr_site *s, uint8_t code, uint64_t g) {
    zr_trail_add(s, code, 0, !(g && g == zr_trail_gen));
}

/* A failure was read: its trail, when the failure added to one, for passing it on. */
static ZR_UNUSED uint64_t zr_trail_claim(void) {
    uint64_t g = zr_trail_live ? zr_trail_gen : 0;
    zr_trail_live = 0;
    return g;
}

/* `@@@t` (DESIGN §7.3): how many entries the trail of a failure with status `st`, read at
 * `s`, has for `t`, after `zr_trail_item` gives them. `*g` is the trail the reader claimed;
 * a failure that added to none, from the library or from a check in the expression itself,
 * starts one where it's read. */
static const zr_site zr_lib_site = {NULL, "zink", 0};
static ZR_UNUSED int64_t zr_trail_take(const zr_site *s, uint8_t st, uint64_t *g) {
    if (!st) return 0;
    if (!(*g && *g == zr_trail_gen)) {
        zr_trail_add(s->fn ? s : &zr_lib_site, st, 0, 1);
        *g = zr_trail_gen;
    }
    zr_trail_live = 0;
    return zr_trail_n < ZR_TRAIL ? zr_trail_n : ZR_TRAIL;
}

/* Entry k of the kept trail, in order, the dropped middle of a long one skipped. */
static ZR_UNUSED zr_trail_entry *zr_trail_item(int64_t k) {
    int64_t n = zr_trail_n;
    return zr_trail_ord(k < ZR_TRAIL_HEAD || n <= ZR_TRAIL ? k : n - (ZR_TRAIL - k));
}

static ZR_UNUSED zr_slice zr_ctext(const char *p) {
    zr_slice r = {(void *)(p ? p : ""), p ? (int64_t)strlen(p) : 0};
    return r;
}

/* The trail's first `upto` entries, before a stop: the header line names the failure
 * `st`, and "it" is that failure here. */
static void zr_trail_print(int64_t upto, uint8_t st) {
    int64_t n = zr_trail_n;
    int64_t tail = n > ZR_TRAIL ? n - (ZR_TRAIL - ZR_TRAIL_HEAD) : ZR_TRAIL_HEAD;
    zr_trail_entry *prev = NULL;
    for (int64_t m = 0; m < upto; m++) {
        if (m >= ZR_TRAIL_HEAD && m < tail) {
            fprintf(stderr, "    ... %lld more\n", (long long)(tail - m));
            m = tail - 1;
            continue;
        }
        zr_trail_entry *e = zr_trail_ord(m);
        const char *what = e->code == st ? "it" : zr_status_name(e->code);
        const char *fn = e->site->fn ? e->site->fn : "?";
        if (e->unread) fprintf(stderr, "    `%s` didn't read %s at %s:%d\n", fn, what, e->site->file, e->site->line);
        else if (prev && prev->code == e->code) fprintf(stderr, "    `%s` passed %s on at %s:%d\n", fn, what, e->site->file, e->site->line);
        else fprintf(stderr, "    `%s` returned %s at %s:%d\n", fn, what, e->site->file, e->site->line);
        prev = e;
    }
}

/* ---- `@@` (DESIGN §7.6): a failure nothing reads jumps to the innermost one, through the
 * defers of the functions it abandons. Each `@@`, and each defer while one is active, is a
 * record on this stack. */
typedef struct zr_catch { struct zr_catch *prev; char *scratch; jmp_buf jb; uint8_t st; } zr_catch;
static ZR_TLS zr_catch *zr_catch_top;
static ZR_TLS char *zr_scratch_top;

static ZR_UNUSED void zr_catch_enter(zr_catch *k) {
    k->prev = zr_catch_top;
    k->scratch = zr_scratch_top;
    k->st = 0;
    zr_catch_top = k;
}

static ZR_UNUSED void zr_catch_leave(zr_catch *k) { zr_catch_top = k->prev; }

/* Jumps to the innermost record, if there is one, with status `st`. */
static ZR_UNUSED void zr_abandon(uint8_t st) {
    zr_catch *k = zr_catch_top;
    if (!k) return;
    zr_catch_top = k->prev;
    zr_scratch_top = k->scratch;
    k->st = st;
    longjmp(k->jb, 1);
}

/* After a defer an abandoned function ran: on to the next record. */
static ZR_NORETURN ZR_UNUSED void zr_abandon_on(uint8_t st) {
    zr_abandon(st);
    zr_fail("zink runtime", 0, "an abandoned call has nowhere to go");
}

/* A check failed and nothing reads it: an `@@` catches it, or the program stops here. */
static ZR_NORETURN ZR_UNUSED void zr_stop(const zr_site *s, uint8_t st, const char *msg) {
    if (s->fn) zr_trail_add(s, st, 1, 1);
    zr_abandon(st);
    zr_flush();
    fprintf(stderr, "%s:%d: %s\n", s->file, s->line, msg);
    exit(1);
}

/* A call failed and nothing read its status: an `@@` catches it, or the program stops,
 * at the call, with the trail that led there. */
static ZR_NORETURN ZR_UNUSED void zr_unread(const zr_site *s, uint8_t st, const char *what) {
    int64_t before = zr_trail_live ? zr_trail_n : 0;
    if (s->fn) zr_trail_add(s, st, 1, !zr_trail_live);
    zr_abandon(st);
    zr_flush();
    fprintf(stderr, "%s:%d: `%s` failed with %s, and nothing read its status\n", s->file, s->line, what, zr_status_name(st));
    zr_trail_print(before, st);
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

static ZR_TLS void *zr_stacks[4096];

static ZR_UNUSED void *zr_stack(int site) {
    if (zr_stacks[site]) return zr_stacks[site];
    void *p = mmap(NULL, ZR_STACK_BYTES, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
    if (p == MAP_FAILED) zr_fail("zink runtime", 0, "can't reserve memory for a data stack");
    zr_stacks[site] = p;
    return p;
}

/* The scratch stack holds temporaries, such as interpolated text passed to a function,
 * which are released at the end of their statement (DESIGN §4.11). */
static ZR_TLS char *zr_scratch_base;

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

/* How far a deque or a cursor array's index may drift from its middle, beyond twice its
 * own size, before it is moved back: a queue that never empties would otherwise creep
 * through its reservation, and past it, one element per operation. Moving it costs its
 * size once per that much drift, so O(1) per operation. */
#define ZR_DRIFT ((int64_t)4 << 20)

/* After a removal from a deque: back to the middle when empty, or when it has drifted
 * far. A removal ends every reference and slice into the array (rule R, REFERENCES §5.2),
 * so its elements may move then. */
static ZR_UNUSED void zr_deq_settle(zr_deq *d, size_t esz) {
    if (d->n == 0) {
        d->p = d->mid;
        return;
    }
    int64_t live = d->n * (int64_t)esz, off = (char *)d->p - (char *)d->mid;
    if (off < 0) off = -off;
    if (off > ZR_DRIFT + 2 * live) {
        char *to = (char *)d->mid - d->n / 2 * (int64_t)esz;
        memmove(to, d->p, (size_t)live);
        d->p = to;
    }
}

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

/* Each half of the index back to its middle when empty, or when it has drifted far. Only
 * slot numbers move: the elements stay where they are, and so do references to them. */
static void zr_cur_settle(zr_cur *c) {
    int64_t far = ZR_DRIFT / 4;
    if (c->bn == 0) c->bp = c->bmid;
    else if (llabs((long long)(c->bp - c->bmid)) > far + 2 * c->bn) {
        uint32_t *to = c->bmid - c->bn / 2;
        memmove(to, c->bp, (size_t)c->bn * 4);
        c->bp = to;
    }
    if (c->an == 0) c->ap = c->amid;
    else if (llabs((long long)(c->ap - c->amid)) > far + 2 * c->an) {
        uint32_t *to = c->amid - c->an / 2;
        memmove(to, c->ap, (size_t)c->an * 4);
        c->ap = to;
    }
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
            uint32_t h = c->heads[d] & c->M;
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
    /* Heads from the larger chunks used so far would be out of a smaller chunk's range. */
    memset(c->heads, 0, (((size_t)c->n >> c->L) + 1) * 4);
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

#if defined(__GLIBC__) && !defined(ZR_THREADS)
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
#if defined(__GLIBC__) && !defined(ZR_TSAN)
    /* Straight into stdio's buffer while it has room, as glibc's own putc_unlocked does
     * through the same fields. Only when fully buffered: a terminal's line buffering
     * needs stdio to see each newline. */
    FILE *f = zr_stdout;
    if (fd == 1 && f) {
        ZR_OUT_LOCK();
        if ((size_t)(f->_IO_write_end - f->_IO_write_ptr) >= n) {
            memcpy(f->_IO_write_ptr, p, n);
            f->_IO_write_ptr += n;
            ZR_OUT_UNLOCK();
            return;
        }
        ZR_OUT_UNLOCK();
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
        zr_copy((char *)s.v->p + s.v->n, p, n);
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
 * for exponents from -6 to 20, as in Java and JavaScript, and scientific beyond. With
 * `single`, the same for a float, which x holds exactly: 0.1f prints as 0.1, not as
 * the double it widens to, 0.10000000149011612. */
static int zr_fmt_float(char *buf, double x, int single) {
    if (isnan(x)) { memcpy(buf, "NaN", 3); return 3; }
    if (isinf(x)) { if (x < 0) { memcpy(buf, "-Infinity", 9); return 9; } memcpy(buf, "Infinity", 8); return 8; }
    if (x == 0) { if (signbit(x)) { memcpy(buf, "-0.0", 4); return 4; } memcpy(buf, "0.0", 3); return 3; }
    int prec = 1;
    for (; prec < (single ? 9 : 17); prec++) {
        snprintf(buf, 40, "%.*e", prec - 1, x);
        if (single ? strtof(buf, NULL) == (float)x : strtod(buf, NULL) == x) break;
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
static int zr_fmt_f64(char *buf, double x) { return zr_fmt_float(buf, x, 0); }
static ZR_UNUSED void zr_w_f64(zr_sink s, double x) { char b[400]; zr_put(s, b, (size_t)zr_fmt_f64(b, x)); }
static ZR_UNUSED void zr_w_f32(zr_sink s, float x) { char b[400]; zr_put(s, b, (size_t)zr_fmt_float(b, x, 1)); }
static ZR_UNUSED void zr_w_i64_pad(zr_sink s, int64_t x, int64_t w, char fill) { char b[24]; zr_pad(s, b, zr_fmt_i64(b, x), w, fill); }
static ZR_UNUSED void zr_w_u64_pad(zr_sink s, uint64_t x, int64_t w, char fill) { char b[24]; zr_pad(s, b, zr_fmt_u64(b, x), w, fill); }
static ZR_UNUSED void zr_w_f64_pad(zr_sink s, double x, int64_t w, char fill) { char b[400]; zr_pad(s, b, zr_fmt_f64(b, x), w, fill); }
static ZR_UNUSED void zr_w_f32_pad(zr_sink s, float x, int64_t w, char fill) { char b[400]; zr_pad(s, b, zr_fmt_float(b, x, 1), w, fill); }

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
    /* The longest is a sign, 309 digits, a point and 100 decimals, and the NUL: 412. */
    char b[420];
    if (d < 0) d = 0;
    if (d > 100) d = 100;
    int n = snprintf(b, sizeof b, "%.*f", (int)d, x);
    if (n < 0) n = 0;
    if ((size_t)n >= sizeof b) n = (int)sizeof b - 1;
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

/* The same for an array of variable-size records, which is its bytes. */
static ZR_UNUSED zr_rvec zr_rvec_scratch(zr_rvec v) {
    zr_rvec c = {zr_scratch_copy(v.p, (size_t)v.bytes), v.n, v.bytes};
    return c;
}

static ZR_UNUSED zr_vec zr_scratch_begin(void) { zr_sm(); zr_vec v = {zr_scratch_top, 0}; return v; }
static ZR_UNUSED zr_slice zr_scratch_end(zr_vec v) { zr_scratch_top = (char *)v.p + v.n; zr_slice s = {v.p, v.n}; return s; }

/* ------------------------------------------------------------ text */

static ZR_UNUSED zr_slice zr_lit(const char *p, int64_t n) { zr_slice s = {(void *)p, n}; return s; }
static ZR_UNUSED zr_slice zr_view(zr_vec v) { zr_slice s = {v.p, v.n}; return s; }

static ZR_UNUSED bool zr_eq(zr_slice a, zr_slice b, size_t elem) {
    return a.n == b.n && zr_memeq(a.p, b.p, (size_t)a.n * elem);
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

static ZR_UNUSED bool zr_starts(zr_slice s, zr_slice x) { return s.n >= x.n && zr_memeq(s.p, x.p, (size_t)x.n); }
static ZR_UNUSED bool zr_ends(zr_slice s, zr_slice x) { return s.n >= x.n && zr_memeq((char *)s.p + s.n - x.n, x.p, (size_t)x.n); }

/* The first x at or after `from`, or -1. memchr finds each place x's first byte occurs,
 * many bytes a step, and only there is the rest compared. */
static int64_t zr_find_from(zr_slice s, zr_slice x, int64_t from) {
    if (x.n == 0) return from;
    const uint8_t *p = (const uint8_t *)s.p, *q = (const uint8_t *)x.p;
    int64_t last = s.n - x.n; /* the last place x could start */
    for (int64_t i = from; i <= last; i++) {
        const uint8_t *f = (const uint8_t *)memchr(p + i, q[0], (size_t)(last - i + 1));
        if (!f) return -1;
        i = f - p;
        if (zr_memeq(f + 1, q + 1, (size_t)x.n - 1)) return i;
    }
    return -1;
}

typedef struct { int64_t v; uint8_t s; } zr_res_i64;
typedef struct { uint64_t v; uint8_t s; } zr_res_u64;
typedef struct { double v; uint8_t s; } zr_res_f64;
typedef struct { zr_i128 v; uint8_t s; } zr_res_i128;
typedef struct { zr_u128 v; uint8_t s; } zr_res_u128;

static ZR_UNUSED zr_res_i64 zr_find(zr_slice s, zr_slice x, int64_t from) {
    if (from < 0 || from > s.n) { zr_res_i64 bad = {0, 22}; return bad; } /* INVALID */
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

/* Strict UTF-8 (RFC 3629): no overlong forms, no surrogates, nothing past U+10FFFF. */
static ZR_UNUSED bool zr_valid_utf8(zr_slice s) {
    const uint8_t *p = (const uint8_t *)s.p;
    int64_t i = 0, n = s.n;
    while (i < n) {
        uint8_t c = p[i];
        if (c < 0x80) { i++; continue; }
        int k;
        uint32_t lo = 0x80, hi = 0xBF;
        if (c >= 0xC2 && c <= 0xDF) k = 1;
        else if (c >= 0xE0 && c <= 0xEF) { k = 2; if (c == 0xE0) lo = 0xA0; if (c == 0xED) hi = 0x9F; }
        else if (c >= 0xF0 && c <= 0xF4) { k = 3; if (c == 0xF0) lo = 0x90; if (c == 0xF4) hi = 0x8F; }
        else return false;
        if (i + k >= n) return false;
        if (p[i + 1] < lo || p[i + 1] > hi) return false;
        for (int j = 2; j <= k; j++) if (p[i + j] < 0x80 || p[i + j] > 0xBF) return false;
        i += k + 1;
    }
    return true;
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

/* Checked arithmetic (DESIGN §9.2). On overflow or division by zero each runs `fail`, which
 * stops the program, or inside `@` sets the status and jumps to where it's read (§7.6). */
#define ZR_ADD(T, a, b, fail) ({ T _r; if (__builtin_add_overflow((T)(a), (T)(b), &_r)) { fail; } _r; })
#define ZR_SUB(T, a, b, fail) ({ T _r; if (__builtin_sub_overflow((T)(a), (T)(b), &_r)) { fail; } _r; })
#define ZR_MUL(T, a, b, fail) ({ T _r; if (__builtin_mul_overflow((T)(a), (T)(b), &_r)) { fail; } _r; })
/* Division wraps where it overflows: `MIN / -1` is `MIN`, as negating it is. Dividing by -1
 * is a negation, since the C division would trap. */
#define ZR_DIV(T, U, a, b, zero) ({ T _a = (a), _b = (b); if (_b == 0) { zero; } \
    ((T)-1 < 0 && _b == (T)-1) ? (T)((U)0 - (U)_a) : (T)(_a / _b); })
#define ZR_REM(T, a, b, zero) ({ T _a = (a), _b = (b); if (_b == 0) { zero; } \
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

/* Checked indexes (DESIGN §4.10): out of range runs `fail`, as the arithmetic does. */
/* Self-relative links (REFERENCES §7.2): a field holding the signed byte offset from
 * itself to what it refers to, in 1, 2 or 4 bytes; in a nullable one, 0 is null. Following
 * one is an add. Storing one runs `fail` when the offset doesn't fit the field, or would
 * be 0 in a nullable one, where it would read back as null (L4). */
#define ZR_LINK(T, f) ({ __typeof__(f) *_lf = &(f); (T *)((char *)_lf + *_lf); })
#define ZR_LINK_OR_NULL(T, f) ({ __typeof__(f) *_lf = &(f); *_lf ? (T *)((char *)_lf + *_lf) : (T *)0; })
#define ZR_LINK_SET(f, t, nullable, fail) ({ __typeof__(f) *_lf = &(f); char *_lt = (char *)(t); \
    int64_t _ld = _lt ? _lt - (char *)_lf : 0; \
    if (_lt && (_ld != (int64_t)(__typeof__(f))_ld || ((nullable) && _ld == 0))) { fail; } \
    *_lf = (__typeof__(f))_ld; (void)0; })

/* Pool links (REFERENCES §7.3): the unsigned byte offset of the target from the start of a
 * static grow-only array, plus one, so that 0 is null; the array's start never moves, so
 * a value holding them can be copied anywhere. Storing one runs `fail` when the target is
 * before the array or the offset doesn't fit the field (L4). */
#define ZR_PLINK(T, f, base) ((T *)((char *)(base) + (f) - 1))
#define ZR_PLINK_OR_NULL(T, f, base) ({ __typeof__(f) _pf = (f); _pf ? (T *)((char *)(base) + _pf - 1) : (T *)0; })
#define ZR_PLINK_SET(f, t, base, fail) ({ __typeof__(f) *_lf = &(f); char *_lt = (char *)(t); \
    uint64_t _ld = _lt ? (uint64_t)(_lt - (char *)(base)) + 1 : 0; \
    if (_lt && (_lt < (char *)(base) || _ld != (uint64_t)(__typeof__(f))_ld)) { fail; } \
    *_lf = (__typeof__(f))_ld; (void)0; })

#define ZR_IX(i, n, fail) ({ int64_t _zxi = (i), _zxn = (n); if (_zxi < 0 || _zxi >= _zxn) { fail; } _zxi; })
#define ZR_IX_WRAP(i, n, fail) ({ int64_t _zxi = (i), _zxn = (n); if (_zxn == 0) { fail; } \
    int64_t _zxr = _zxi % _zxn; _zxr < 0 ? _zxr + _zxn : _zxr; })
#define ZR_IX_CLAMP(i, n, fail) ({ int64_t _zxi = (i), _zxn = (n); if (_zxn == 0) { fail; } \
    _zxi < 0 ? 0 : _zxi >= _zxn ? _zxn - 1 : _zxi; })

static ZR_NORETURN ZR_UNUSED void zr_ix_fail(const zr_site *s, int64_t i, int64_t n) {
    char msg[96];
    snprintf(msg, sizeof msg, "index %lld is out of range for length %lld", (long long)i, (long long)n);
    zr_stop(s, 34, msg);
}

static ZR_NORETURN ZR_UNUSED void zr_range_fail(const zr_site *s, int64_t i, int64_t j, int64_t n) {
    char msg[96];
    snprintf(msg, sizeof msg, "slice %lld:%lld is out of range for length %lld", (long long)i, (long long)j, (long long)n);
    zr_stop(s, 34, msg);
}

/* What a reference caught by `@` refers to when the expression failed: a zero value of its
 * type, of its own, since there is no element (DESIGN §7.5). */
static ZR_UNUSED void *zr_zero_ref(size_t n) {
    void *p = calloc(1, n ? n : 1);
    if (!p) zr_fail("zink runtime", 0, "out of memory for a zero value");
    return p;
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

/* ------------------------------------------------------------ environment (STDLIB §2.3) */

extern char **environ;
static uint8_t zr_errno(void);

#ifdef ZR_THREADS
static pthread_mutex_t zr_env_mu = PTHREAD_MUTEX_INITIALIZER;
#define ZR_ENV_LOCK() pthread_mutex_lock(&zr_env_mu)
#define ZR_ENV_UNLOCK() pthread_mutex_unlock(&zr_env_mu)
#else
#define ZR_ENV_LOCK() ((void)0)
#define ZR_ENV_UNLOCK() ((void)0)
#endif

/* A view of the variable's value, which the runtime never frees, even after env.set
 * replaces it; p is NULL when it isn't set. */
static ZR_UNUSED zr_slice zr_env_get(zr_slice name) {
    int64_t m = zr_sm();
    ZR_ENV_LOCK();
    const char *v = getenv(zr_cstr(name));
    ZR_ENV_UNLOCK();
    zr_sr(m);
    zr_slice s = {(void *)v, v ? (int64_t)strlen(v) : 0};
    return s;
}

/* INVALID for an empty name or one with '='. glibc keeps the text it replaces. */
static ZR_UNUSED uint8_t zr_env_set(zr_slice name, zr_slice value, int32_t unset) {
    if (name.n == 0 || memchr(name.p, '=', (size_t)name.n)) return 22;
    int64_t m = zr_sm();
    const char *n = zr_cstr(name);
    const char *v = unset ? NULL : zr_cstr(value);
    ZR_ENV_LOCK();
    int r = unset ? unsetenv(n) : setenv(n, v, 1);
    ZR_ENV_UNLOCK();
    uint8_t s = r < 0 ? zr_errno() : 0;
    zr_sr(m);
    return s;
}

/* The i-th "name=value" of the environment, or an empty view past the end. */
static ZR_UNUSED zr_slice zr_env_entry(int64_t i) {
    ZR_ENV_LOCK();
    const char *e = NULL;
    int64_t k = 0;
    while (k < i && environ[k]) k++;
    if (k == i) e = environ[i];
    ZR_ENV_UNLOCK();
    zr_slice s = {(void *)e, e ? (int64_t)strlen(e) : 0};
    return s;
}

/* ------------------------------------------------------------ hashing (STDLIB §8.3) */

static ZR_UNUSED uint64_t zr_fnv1a(zr_slice s) {
    uint64_t h = 0xcbf29ce484222325ULL;
    const uint8_t *p = (const uint8_t *)s.p;
    for (int64_t i = 0; i < s.n; i++) { h ^= p[i]; h *= 0x100000001b3ULL; }
    return h;
}

/* CRC-32 with the IEEE polynomial, reflected, as gzip, zip and zlib compute it. `crc` is
 * the value so far, 0 to start. */
static ZR_UNUSED uint32_t zr_crc32(uint32_t crc, zr_slice s) {
    static uint32_t table[256];
    if (!table[1]) {
        for (uint32_t i = 0; i < 256; i++) {
            uint32_t c = i;
            for (int k = 0; k < 8; k++) c = c & 1 ? 0xEDB88320u ^ (c >> 1) : c >> 1;
            table[i] = c;
        }
    }
    const uint8_t *p = (const uint8_t *)s.p;
    crc = ~crc;
    for (int64_t i = 0; i < s.n; i++) crc = table[(crc ^ p[i]) & 0xFF] ^ (crc >> 8);
    return ~crc;
}

/* SHA-2 (FIPS 180-4) and SHA-3 (FIPS 202). A hash object keeps this state in its own
 * bytes, with the algorithm in it, so one init, update and final serve all twelve:
 *   1 SHA-224    2 SHA-256    3 SHA-384    4 SHA-512    5 SHA-512/224    6 SHA-512/256
 *   7 SHA3-224   8 SHA3-256   9 SHA3-384  10 SHA3-512  11 SHAKE128       12 SHAKE256
 * The constants are derived from their definitions (cube and square roots of primes, the
 * SHA-512/t IV function, Keccak's LFSR) and the results checked against Python's hashlib. */
typedef struct {
    uint32_t kind, used;     /* the algorithm; bytes waiting in buf, or absorbed into a */
    uint64_t len;            /* bytes so far (SHA-2) */
    union {
        struct { uint32_t h[8]; uint8_t buf[64]; } s2;
        struct { uint64_t h[8]; uint8_t buf[128]; } s5;
        uint64_t a[25];
    } u;
} zr_digest;

#define ZR_DIGEST_SIZE 216
typedef char zr_digest_fits[sizeof(zr_digest) == ZR_DIGEST_SIZE ? 1 : -1];

static const uint32_t zr_sha2_k[64] = {
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2};
static const uint32_t zr_sha2_iv256[8] = {
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19};
static const uint64_t zr_sha5_k[80] = {
    0x428a2f98d728ae22ULL, 0x7137449123ef65cdULL, 0xb5c0fbcfec4d3b2fULL, 0xe9b5dba58189dbbcULL,
    0x3956c25bf348b538ULL, 0x59f111f1b605d019ULL, 0x923f82a4af194f9bULL, 0xab1c5ed5da6d8118ULL,
    0xd807aa98a3030242ULL, 0x12835b0145706fbeULL, 0x243185be4ee4b28cULL, 0x550c7dc3d5ffb4e2ULL,
    0x72be5d74f27b896fULL, 0x80deb1fe3b1696b1ULL, 0x9bdc06a725c71235ULL, 0xc19bf174cf692694ULL,
    0xe49b69c19ef14ad2ULL, 0xefbe4786384f25e3ULL, 0x0fc19dc68b8cd5b5ULL, 0x240ca1cc77ac9c65ULL,
    0x2de92c6f592b0275ULL, 0x4a7484aa6ea6e483ULL, 0x5cb0a9dcbd41fbd4ULL, 0x76f988da831153b5ULL,
    0x983e5152ee66dfabULL, 0xa831c66d2db43210ULL, 0xb00327c898fb213fULL, 0xbf597fc7beef0ee4ULL,
    0xc6e00bf33da88fc2ULL, 0xd5a79147930aa725ULL, 0x06ca6351e003826fULL, 0x142929670a0e6e70ULL,
    0x27b70a8546d22ffcULL, 0x2e1b21385c26c926ULL, 0x4d2c6dfc5ac42aedULL, 0x53380d139d95b3dfULL,
    0x650a73548baf63deULL, 0x766a0abb3c77b2a8ULL, 0x81c2c92e47edaee6ULL, 0x92722c851482353bULL,
    0xa2bfe8a14cf10364ULL, 0xa81a664bbc423001ULL, 0xc24b8b70d0f89791ULL, 0xc76c51a30654be30ULL,
    0xd192e819d6ef5218ULL, 0xd69906245565a910ULL, 0xf40e35855771202aULL, 0x106aa07032bbd1b8ULL,
    0x19a4c116b8d2d0c8ULL, 0x1e376c085141ab53ULL, 0x2748774cdf8eeb99ULL, 0x34b0bcb5e19b48a8ULL,
    0x391c0cb3c5c95a63ULL, 0x4ed8aa4ae3418acbULL, 0x5b9cca4f7763e373ULL, 0x682e6ff3d6b2b8a3ULL,
    0x748f82ee5defb2fcULL, 0x78a5636f43172f60ULL, 0x84c87814a1f0ab72ULL, 0x8cc702081a6439ecULL,
    0x90befffa23631e28ULL, 0xa4506cebde82bde9ULL, 0xbef9a3f7b2c67915ULL, 0xc67178f2e372532bULL,
    0xca273eceea26619cULL, 0xd186b8c721c0c207ULL, 0xeada7dd6cde0eb1eULL, 0xf57d4f7fee6ed178ULL,
    0x06f067aa72176fbaULL, 0x0a637dc5a2c898a6ULL, 0x113f9804bef90daeULL, 0x1b710b35131c471bULL,
    0x28db77f523047d84ULL, 0x32caab7b40c72493ULL, 0x3c9ebe0a15c9bebcULL, 0x431d67c49c100d4cULL,
    0x4cc5d4becb3e42b6ULL, 0x597f299cfc657e2aULL, 0x5fcb6fab3ad6faecULL, 0x6c44198c4a475817ULL};
static const uint64_t zr_sha5_iv512[8] = {
    0x6a09e667f3bcc908ULL, 0xbb67ae8584caa73bULL, 0x3c6ef372fe94f82bULL, 0xa54ff53a5f1d36f1ULL,
    0x510e527fade682d1ULL, 0x9b05688c2b3e6c1fULL, 0x1f83d9abfb41bd6bULL, 0x5be0cd19137e2179ULL};
static const uint64_t zr_sha5_iv384[8] = {
    0xcbbb9d5dc1059ed8ULL, 0x629a292a367cd507ULL, 0x9159015a3070dd17ULL, 0x152fecd8f70e5939ULL,
    0x67332667ffc00b31ULL, 0x8eb44a8768581511ULL, 0xdb0c2e0d64f98fa7ULL, 0x47b5481dbefa4fa4ULL};
static const uint64_t zr_sha5_iv224[8] = {
    0x8c3d37c819544da2ULL, 0x73e1996689dcd4d6ULL, 0x1dfab7ae32ff9c82ULL, 0x679dd514582f9fcfULL,
    0x0f6d2b697bd44da8ULL, 0x77e36f7304c48942ULL, 0x3f9d85a86a1d36c8ULL, 0x1112e6ad91d692a1ULL};
static const uint64_t zr_sha5_iv256[8] = {
    0x22312194fc2bf72cULL, 0x9f555fa3c84c64c2ULL, 0x2393b86b6f53b151ULL, 0x963877195940eabdULL,
    0x96283ee2a88effe3ULL, 0xbe5e1e2553863992ULL, 0x2b0199fc2c85b8aaULL, 0x0eb72ddc81c52ca2ULL};
static const uint32_t zr_sha2_iv224[8] = {
    0xc1059ed8, 0x367cd507, 0x3070dd17, 0xf70e5939, 0xffc00b31, 0x68581511, 0x64f98fa7, 0xbefa4fa4};
static const uint64_t zr_keccak_rc[24] = {
    0x0000000000000001ULL, 0x0000000000008082ULL, 0x800000000000808aULL, 0x8000000080008000ULL,
    0x000000000000808bULL, 0x0000000080000001ULL, 0x8000000080008081ULL, 0x8000000000008009ULL,
    0x000000000000008aULL, 0x0000000000000088ULL, 0x0000000080008009ULL, 0x000000008000000aULL,
    0x000000008000808bULL, 0x800000000000008bULL, 0x8000000000008089ULL, 0x8000000000008003ULL,
    0x8000000000008002ULL, 0x8000000000000080ULL, 0x000000000000800aULL, 0x800000008000000aULL,
    0x8000000080008081ULL, 0x8000000000008080ULL, 0x0000000080000001ULL, 0x8000000080008008ULL};

#define ZR_ROR32(x, n) (((x) >> (n)) | ((x) << (32 - (n))))
#define ZR_ROR64(x, n) (((x) >> (n)) | ((x) << (64 - (n))))
#define ZR_ROL64(x, n) (((x) << (n)) | ((x) >> (64 - (n))))

static void zr_sha2_block(uint32_t *h, const uint8_t *b) {
    uint32_t w[64];
    for (int i = 0; i < 16; i++)
        w[i] = (uint32_t)b[4 * i] << 24 | (uint32_t)b[4 * i + 1] << 16 | (uint32_t)b[4 * i + 2] << 8 | b[4 * i + 3];
    for (int i = 16; i < 64; i++) {
        uint32_t s0 = ZR_ROR32(w[i - 15], 7) ^ ZR_ROR32(w[i - 15], 18) ^ (w[i - 15] >> 3);
        uint32_t s1 = ZR_ROR32(w[i - 2], 17) ^ ZR_ROR32(w[i - 2], 19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }
    uint32_t a = h[0], bb = h[1], c = h[2], d = h[3], e = h[4], f = h[5], g = h[6], hh = h[7];
    for (int i = 0; i < 64; i++) {
        uint32_t t1 = hh + (ZR_ROR32(e, 6) ^ ZR_ROR32(e, 11) ^ ZR_ROR32(e, 25)) + ((e & f) ^ (~e & g)) + zr_sha2_k[i] + w[i];
        uint32_t t2 = (ZR_ROR32(a, 2) ^ ZR_ROR32(a, 13) ^ ZR_ROR32(a, 22)) + ((a & bb) ^ (a & c) ^ (bb & c));
        hh = g; g = f; f = e; e = d + t1; d = c; c = bb; bb = a; a = t1 + t2;
    }
    h[0] += a; h[1] += bb; h[2] += c; h[3] += d; h[4] += e; h[5] += f; h[6] += g; h[7] += hh;
}

static void zr_sha5_block(uint64_t *h, const uint8_t *b) {
    uint64_t w[80];
    for (int i = 0; i < 16; i++) {
        uint64_t x = 0;
        for (int k = 0; k < 8; k++) x = x << 8 | b[8 * i + k];
        w[i] = x;
    }
    for (int i = 16; i < 80; i++) {
        uint64_t s0 = ZR_ROR64(w[i - 15], 1) ^ ZR_ROR64(w[i - 15], 8) ^ (w[i - 15] >> 7);
        uint64_t s1 = ZR_ROR64(w[i - 2], 19) ^ ZR_ROR64(w[i - 2], 61) ^ (w[i - 2] >> 6);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }
    uint64_t a = h[0], bb = h[1], c = h[2], d = h[3], e = h[4], f = h[5], g = h[6], hh = h[7];
    for (int i = 0; i < 80; i++) {
        uint64_t t1 = hh + (ZR_ROR64(e, 14) ^ ZR_ROR64(e, 18) ^ ZR_ROR64(e, 41)) + ((e & f) ^ (~e & g)) + zr_sha5_k[i] + w[i];
        uint64_t t2 = (ZR_ROR64(a, 28) ^ ZR_ROR64(a, 34) ^ ZR_ROR64(a, 39)) + ((a & bb) ^ (a & c) ^ (bb & c));
        hh = g; g = f; f = e; e = d + t1; d = c; c = bb; bb = a; a = t1 + t2;
    }
    h[0] += a; h[1] += bb; h[2] += c; h[3] += d; h[4] += e; h[5] += f; h[6] += g; h[7] += hh;
}

/* Keccak-f[1600]: 24 rounds of theta, rho and pi, chi, iota, unrolled, with the state in
 * locals. The rotations and the lanes' new places are FIPS 202's, worked out once. */
static void zr_keccak_f(uint64_t *st) {
    uint64_t a0 = st[0], a1 = st[1], a2 = st[2], a3 = st[3], a4 = st[4];
    uint64_t a5 = st[5], a6 = st[6], a7 = st[7], a8 = st[8], a9 = st[9];
    uint64_t a10 = st[10], a11 = st[11], a12 = st[12], a13 = st[13], a14 = st[14];
    uint64_t a15 = st[15], a16 = st[16], a17 = st[17], a18 = st[18], a19 = st[19];
    uint64_t a20 = st[20], a21 = st[21], a22 = st[22], a23 = st[23], a24 = st[24];
    for (int r = 0; r < 24; r++) {
        uint64_t c0 = a0 ^ a5 ^ a10 ^ a15 ^ a20, c1 = a1 ^ a6 ^ a11 ^ a16 ^ a21, c2 = a2 ^ a7 ^ a12 ^ a17 ^ a22, c3 = a3 ^ a8 ^ a13 ^ a18 ^ a23, c4 = a4 ^ a9 ^ a14 ^ a19 ^ a24;
        uint64_t d0 = c4 ^ ZR_ROL64(c1, 1), d1 = c0 ^ ZR_ROL64(c2, 1), d2 = c1 ^ ZR_ROL64(c3, 1), d3 = c2 ^ ZR_ROL64(c4, 1), d4 = c3 ^ ZR_ROL64(c0, 1);
        uint64_t b0 = a0 ^ d0, b1 = ZR_ROL64(a6 ^ d1, 44), b2 = ZR_ROL64(a12 ^ d2, 43), b3 = ZR_ROL64(a18 ^ d3, 21), b4 = ZR_ROL64(a24 ^ d4, 14);
        uint64_t b5 = ZR_ROL64(a3 ^ d3, 28), b6 = ZR_ROL64(a9 ^ d4, 20), b7 = ZR_ROL64(a10 ^ d0, 3), b8 = ZR_ROL64(a16 ^ d1, 45), b9 = ZR_ROL64(a22 ^ d2, 61);
        uint64_t b10 = ZR_ROL64(a1 ^ d1, 1), b11 = ZR_ROL64(a7 ^ d2, 6), b12 = ZR_ROL64(a13 ^ d3, 25), b13 = ZR_ROL64(a19 ^ d4, 8), b14 = ZR_ROL64(a20 ^ d0, 18);
        uint64_t b15 = ZR_ROL64(a4 ^ d4, 27), b16 = ZR_ROL64(a5 ^ d0, 36), b17 = ZR_ROL64(a11 ^ d1, 10), b18 = ZR_ROL64(a17 ^ d2, 15), b19 = ZR_ROL64(a23 ^ d3, 56);
        uint64_t b20 = ZR_ROL64(a2 ^ d2, 62), b21 = ZR_ROL64(a8 ^ d3, 55), b22 = ZR_ROL64(a14 ^ d4, 39), b23 = ZR_ROL64(a15 ^ d0, 41), b24 = ZR_ROL64(a21 ^ d1, 2);
        a0 = b0 ^ (~b1 & b2);
        a1 = b1 ^ (~b2 & b3);
        a2 = b2 ^ (~b3 & b4);
        a3 = b3 ^ (~b4 & b0);
        a4 = b4 ^ (~b0 & b1);
        a5 = b5 ^ (~b6 & b7);
        a6 = b6 ^ (~b7 & b8);
        a7 = b7 ^ (~b8 & b9);
        a8 = b8 ^ (~b9 & b5);
        a9 = b9 ^ (~b5 & b6);
        a10 = b10 ^ (~b11 & b12);
        a11 = b11 ^ (~b12 & b13);
        a12 = b12 ^ (~b13 & b14);
        a13 = b13 ^ (~b14 & b10);
        a14 = b14 ^ (~b10 & b11);
        a15 = b15 ^ (~b16 & b17);
        a16 = b16 ^ (~b17 & b18);
        a17 = b17 ^ (~b18 & b19);
        a18 = b18 ^ (~b19 & b15);
        a19 = b19 ^ (~b15 & b16);
        a20 = b20 ^ (~b21 & b22);
        a21 = b21 ^ (~b22 & b23);
        a22 = b22 ^ (~b23 & b24);
        a23 = b23 ^ (~b24 & b20);
        a24 = b24 ^ (~b20 & b21);
        a0 ^= zr_keccak_rc[r];
    }
    st[0] = a0; st[1] = a1; st[2] = a2; st[3] = a3; st[4] = a4;
    st[5] = a5; st[6] = a6; st[7] = a7; st[8] = a8; st[9] = a9;
    st[10] = a10; st[11] = a11; st[12] = a12; st[13] = a13; st[14] = a14;
    st[15] = a15; st[16] = a16; st[17] = a17; st[18] = a18; st[19] = a19;
    st[20] = a20; st[21] = a21; st[22] = a22; st[23] = a23; st[24] = a24;
}

/* Keccak's rate in bytes: what each permutation absorbs or gives out. */
static uint32_t zr_keccak_rate(uint32_t kind) {
    static const uint8_t rate[6] = {144, 136, 104, 72, 168, 136};
    return rate[kind - 7];
}

static ZR_UNUSED void zr_digest_init(zr_slice st, int64_t kind) {
    zr_digest s;
    memset(&s, 0, sizeof s);
    s.kind = (uint32_t)kind;
    switch (kind) {
        case 1: memcpy(s.u.s2.h, zr_sha2_iv224, sizeof s.u.s2.h); break;
        case 2: memcpy(s.u.s2.h, zr_sha2_iv256, sizeof s.u.s2.h); break;
        case 3: memcpy(s.u.s5.h, zr_sha5_iv384, sizeof s.u.s5.h); break;
        case 4: memcpy(s.u.s5.h, zr_sha5_iv512, sizeof s.u.s5.h); break;
        case 5: memcpy(s.u.s5.h, zr_sha5_iv224, sizeof s.u.s5.h); break;
        case 6: memcpy(s.u.s5.h, zr_sha5_iv256, sizeof s.u.s5.h); break;
        default: break;
    }
    memcpy(st.p, &s, sizeof s);
}

/* Feeds bytes to a SHA-2 state of the given block size, whole blocks straight from p. */
#define ZR_SHA_ABSORB(blk, buf, block, h)                                          \
    do {                                                                           \
        if (s->used) {                                                             \
            int64_t take = (blk) - s->used < n ? (blk) - s->used : n;              \
            memcpy((buf) + s->used, p, (size_t)take);                              \
            s->used += (uint32_t)take; p += take; n -= take;                       \
            if (s->used == (blk)) { block(h, buf); s->used = 0; }                  \
        }                                                                          \
        while (n >= (blk)) { block(h, p); p += (blk); n -= (blk); }               \
        if (n) { memcpy(buf, p, (size_t)n); s->used = (uint32_t)n; }               \
    } while (0)

static void zr_digest_absorb(zr_digest *s, const uint8_t *p, int64_t n) {
    if (s->kind <= 2) {
        s->len += (uint64_t)n;
        ZR_SHA_ABSORB(64, s->u.s2.buf, zr_sha2_block, s->u.s2.h);
    } else if (s->kind <= 6) {
        s->len += (uint64_t)n;
        ZR_SHA_ABSORB(128, s->u.s5.buf, zr_sha5_block, s->u.s5.h);
    } else {
        uint32_t rate = zr_keccak_rate(s->kind);
        while (n > 0) {
            if (s->used == 0 && n >= rate) {
                /* A whole block, a lane at a time; the shifts compile to one load. */
                for (uint32_t i = 0; i < rate / 8; i++) {
                    const uint8_t *q = p + 8 * i;
                    s->u.a[i] ^= (uint64_t)q[0] | (uint64_t)q[1] << 8 | (uint64_t)q[2] << 16 | (uint64_t)q[3] << 24 |
                                 (uint64_t)q[4] << 32 | (uint64_t)q[5] << 40 | (uint64_t)q[6] << 48 | (uint64_t)q[7] << 56;
                }
                zr_keccak_f(s->u.a);
                p += rate;
                n -= rate;
                continue;
            }
            s->u.a[s->used >> 3] ^= (uint64_t)*p++ << (8 * (s->used & 7));
            n--;
            if (++s->used == rate) { zr_keccak_f(s->u.a); s->used = 0; }
        }
    }
}

static ZR_UNUSED void zr_digest_update(zr_slice st, zr_slice data) {
    zr_digest s;
    memcpy(&s, st.p, sizeof s);
    zr_digest_absorb(&s, (const uint8_t *)data.p, data.n);
    memcpy(st.p, &s, sizeof s);
}

/* The digest's bytes, out->n of them, appended to out; the state is used up. */
static void zr_digest_end(zr_digest *s, uint8_t *o, int64_t n) {
    if (s->kind <= 6) {
        int big = s->kind > 2;
        int blk = big ? 128 : 64, lenb = big ? 16 : 8;
        uint64_t hi = s->len >> 61, lo = s->len << 3;
        uint8_t pad[144] = {0x80};
        int64_t padn = (int64_t)s->used < blk - lenb ? blk - lenb - s->used : 2 * blk - lenb - s->used;
        for (int i = 0; i < 8; i++) pad[padn + lenb - 1 - i] = (uint8_t)(lo >> (8 * i));
        if (big) for (int i = 0; i < 8; i++) pad[padn + 7 - i] = (uint8_t)(hi >> (8 * i));
        zr_digest_absorb(s, pad, padn + lenb);
        for (int64_t i = 0; i < n; i++)
            o[i] = big ? (uint8_t)(s->u.s5.h[i / 8] >> (56 - 8 * (i % 8)))
                       : (uint8_t)(s->u.s2.h[i / 4] >> (24 - 8 * (i % 4)));
        return;
    }
    uint32_t rate = zr_keccak_rate(s->kind);
    uint8_t domain = s->kind >= 11 ? 0x1f : 0x06;
    s->u.a[s->used >> 3] ^= (uint64_t)domain << (8 * (s->used & 7));
    s->u.a[(rate - 1) >> 3] ^= (uint64_t)0x80 << (8 * ((rate - 1) & 7));
    zr_keccak_f(s->u.a);
    for (int64_t i = 0, at = 0; i < n; i++, at++) {
        if (at == rate) { zr_keccak_f(s->u.a); at = 0; }
        o[i] = (uint8_t)(s->u.a[at >> 3] >> (8 * (at & 7)));
    }
}

/* A fixed-size digest into out, whose length is the digest's. */
static ZR_UNUSED void zr_digest_final(zr_slice st, zr_slice out) {
    zr_digest s;
    memcpy(&s, st.p, sizeof s);
    zr_digest_end(&s, (uint8_t *)out.p, out.n);
}

/* SHAKE's n bytes, appended to out. */
static ZR_UNUSED void zr_digest_squeeze(zr_slice st, int64_t n, zr_vec *out) {
    zr_digest s;
    memcpy(&s, st.p, sizeof s);
    zr_digest_end(&s, (uint8_t *)out->p + out->n, n);
    out->n += n;
}
#undef ZR_SHA_ABSORB

/* ------------------------------------------------------------ simd functions (DESIGN §12.8) */

/* A `simd` function is compiled three times: for baseline x86-64 under its own name, and
 * for AVX2 and for AVX-512 as NAME_simd1 and NAME_simd2. The baseline calls the widest the
 * CPU has. Only clang on x86-64 builds the wider ones; elsewhere the baseline is all there
 * is. Floats are never contracted, so a multiply and an add stay two roundings in the
 * versions with FMA and give the baseline's results. */
#if defined(__clang__) && defined(__x86_64__)
#define ZR_SIMD 1
#pragma clang fp contract(off)
#define ZR_SIMD1 __attribute__((target("avx2,fma,bmi,bmi2,popcnt")))
#define ZR_SIMD2 \
    __attribute__((target("avx512f,avx512bw,avx512cd,avx512dq,avx512vl,avx2,fma,bmi,bmi2,popcnt")))
/* The widest vector unit this CPU and its operating system support: 2 for AVX-512 (F, BW,
 * CD, DQ, VL), 1 for AVX2 (with FMA, BMI1, BMI2 and POPCNT), 0 for neither; asked once. */
static ZR_UNUSED int zr_simd_level(void) {
    static int level = -1;
    if (level < 0) {
        int v3 = __builtin_cpu_supports("avx2") && __builtin_cpu_supports("fma") &&
                 __builtin_cpu_supports("bmi") && __builtin_cpu_supports("bmi2") &&
                 __builtin_cpu_supports("popcnt");
        int v4 = v3 && __builtin_cpu_supports("avx512f") && __builtin_cpu_supports("avx512bw") &&
                 __builtin_cpu_supports("avx512cd") && __builtin_cpu_supports("avx512dq") &&
                 __builtin_cpu_supports("avx512vl");
        level = v4 ? 2 : v3 ? 1 : 0;
    }
    return level;
}
#endif

/* ------------------------------------------------------------ base64 (STDLIB §8.4) */

/* RFC 4648 base64, standard (`url` 0, with padding) and URL-safe (`url` 1, without). Each
 * loop handles whole groups and has no exits, so the C compiler can vectorize it: invalid
 * characters are ORed into a value checked once at the end, as Goose's std base64 does.
 * With clang on an x86-64 CPU that has AVX2 or AVX-512, the loops compute the alphabet
 * with compares and run as that vector code, chosen when the program first asks: two to
 * three times as fast as table lookups, which is what they do elsewhere, being the
 * fastest scalar form. */
static const char zr_b64_abc[2][65] = {
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_",
};
/* Each byte's 6-bit value, 255 for one outside the alphabet. */
static const uint8_t zr_b64_val[2][256] = {
    {
      255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
      255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
      255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,  62, 255, 255, 255,  63,
       52,  53,  54,  55,  56,  57,  58,  59,  60,  61, 255, 255, 255, 255, 255, 255,
      255,   0,   1,   2,   3,   4,   5,   6,   7,   8,   9,  10,  11,  12,  13,  14,
       15,  16,  17,  18,  19,  20,  21,  22,  23,  24,  25, 255, 255, 255, 255, 255,
      255,  26,  27,  28,  29,  30,  31,  32,  33,  34,  35,  36,  37,  38,  39,  40,
       41,  42,  43,  44,  45,  46,  47,  48,  49,  50,  51, 255, 255, 255, 255, 255,
     [128 ... 255] = 255,
    },
    {
      255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
      255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
      255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,  62, 255, 255,
       52,  53,  54,  55,  56,  57,  58,  59,  60,  61, 255, 255, 255, 255, 255, 255,
      255,   0,   1,   2,   3,   4,   5,   6,   7,   8,   9,  10,  11,  12,  13,  14,
       15,  16,  17,  18,  19,  20,  21,  22,  23,  24,  25, 255, 255, 255, 255,  63,
      255,  26,  27,  28,  29,  30,  31,  32,  33,  34,  35,  36,  37,  38,  39,  40,
       41,  42,  43,  44,  45,  46,  47,  48,  49,  50,  51, 255, 255, 255, 255, 255,
     [128 ... 255] = 255,
    },
};

#define ZR_B64_ENC_LOOP(SYM)                                                              \
    for (int64_t i = 0; i < g; i++) {                                                     \
        uint8_t a = s[3 * i], b = s[3 * i + 1], c = s[3 * i + 2];                         \
        d[4 * i] = SYM(a >> 2);                                                           \
        d[4 * i + 1] = SYM((uint8_t)((a & 3) << 4 | b >> 4));                             \
        d[4 * i + 2] = SYM((uint8_t)((b & 15) << 2 | c >> 6));                            \
        d[4 * i + 3] = SYM((uint8_t)(c & 63));                                            \
    }
#define ZR_B64_DEC_LOOP(VAL)                                                              \
    uint8_t seen = 0;                                                                     \
    for (int64_t i = 0; i < g; i++) {                                                     \
        uint8_t a = VAL(t[4 * i]), b = VAL(t[4 * i + 1]);                                 \
        uint8_t c = VAL(t[4 * i + 2]), e = VAL(t[4 * i + 3]);                             \
        seen |= a | b | c | e;                                                            \
        o[3 * i] = (uint8_t)(a << 2 | b >> 4);                                            \
        o[3 * i + 1] = (uint8_t)(b << 4 | c >> 2);                                        \
        o[3 * i + 2] = (uint8_t)(c << 6 | e);                                             \
    }                                                                                     \
    return seen;

static void zr_b64_enc_table(uint8_t *restrict d, const uint8_t *restrict s, int64_t g, int url) {
    const char *abc = zr_b64_abc[url];
#define ZR_B64_SYM(x) ((uint8_t)abc[x])
    ZR_B64_ENC_LOOP(ZR_B64_SYM)
#undef ZR_B64_SYM
}

static uint8_t zr_b64_dec_table(uint8_t *restrict o, const uint8_t *restrict t, int64_t g, int url) {
    const uint8_t *val = zr_b64_val[url];
#define ZR_B64_VAL(x) val[x]
    ZR_B64_DEC_LOOP(ZR_B64_VAL)
#undef ZR_B64_VAL
}

#if defined(__clang__) && defined(__x86_64__)
#define ZR_B64_SIMD 1
/* The character of a 6-bit value: an offset added to it, chosen by range. */
static inline __attribute__((always_inline)) uint8_t zr_b64_sym(uint8_t b, int url) {
    uint8_t d = 'A';
    if (b >= 26) d = 'a' - 26;
    if (b >= 52) d = (uint8_t)('0' - 52);
    if (b == 62) d = (uint8_t)((url ? '-' : '+') - 62);
    if (b == 63) d = (uint8_t)((url ? '_' : '/') - 63);
    return (uint8_t)(b + d);
}
/* The 6-bit value of a character, or 255: each range test is a subtraction and a compare.
 * The value adds to the byte the test computed, (uint8_t)(c - 'a') + 26: written as
 * c - 'a' + 26 instead, the decode loop clang vectorizes took about 1.5 times as long. */
static inline __attribute__((always_inline)) uint8_t zr_b64_value(uint8_t c, int url) {
    uint8_t v = 255;
    if ((uint8_t)(c - 'A') < 26) v = (uint8_t)(c - 'A');
    if ((uint8_t)(c - 'a') < 26) v = (uint8_t)((uint8_t)(c - 'a') + 26);
    if ((uint8_t)(c - '0') < 10) v = (uint8_t)((uint8_t)(c - '0') + 52);
    if (c == (url ? '-' : '+')) v = 62;
    if (c == (url ? '_' : '/')) v = 63;
    return v;
}
#define ZR_B64_SYM_STD(x) zr_b64_sym(x, 0)
#define ZR_B64_SYM_URL(x) zr_b64_sym(x, 1)
#define ZR_B64_VAL_STD(x) zr_b64_value(x, 0)
#define ZR_B64_VAL_URL(x) zr_b64_value(x, 1)
/* The loops compiled for one instruction set, each alphabet's constants folded in. */
#define ZR_B64_FNS(NAME, TARGET)                                                          \
    __attribute__((target(TARGET))) static void zr_b64_enc_##NAME(                       \
        uint8_t *restrict d, const uint8_t *restrict s, int64_t g, int url) {             \
        if (url) { ZR_B64_ENC_LOOP(ZR_B64_SYM_URL) } else { ZR_B64_ENC_LOOP(ZR_B64_SYM_STD) } \
    }                                                                                     \
    __attribute__((target(TARGET))) static uint8_t zr_b64_dec_##NAME(                    \
        uint8_t *restrict o, const uint8_t *restrict t, int64_t g, int url) {             \
        if (url) { ZR_B64_DEC_LOOP(ZR_B64_VAL_URL) } else { ZR_B64_DEC_LOOP(ZR_B64_VAL_STD) } \
    }
ZR_B64_FNS(avx2, "avx2,bmi,bmi2")
ZR_B64_FNS(avx512, "avx512f,avx512bw,avx512vl,avx512dq,avx512cd,avx2,bmi,bmi2")
#undef ZR_B64_FNS
#undef ZR_B64_SYM_STD
#undef ZR_B64_SYM_URL
#undef ZR_B64_VAL_STD
#undef ZR_B64_VAL_URL
#endif
#undef ZR_B64_ENC_LOOP
#undef ZR_B64_DEC_LOOP

/* Appends the text of `bytes` to `out`. */
static ZR_UNUSED void zr_b64_encode(zr_slice bytes, int32_t url, zr_vec *out) {
    const uint8_t *s = (const uint8_t *)bytes.p;
    int64_t g = bytes.n / 3, rest = bytes.n % 3;
    uint8_t *d = (uint8_t *)out->p + out->n;
#ifdef ZR_B64_SIMD
    int level = zr_simd_level();
    if (level == 2) zr_b64_enc_avx512(d, s, g, url);
    else if (level == 1) zr_b64_enc_avx2(d, s, g, url);
    else
#endif
        zr_b64_enc_table(d, s, g, url);
    s += 3 * g;
    d += 4 * g;
    int64_t n = 4 * g;
    if (rest > 0) {
        const char *abc = zr_b64_abc[url];
        uint8_t a = s[0], b = rest == 2 ? s[1] : 0;
        d[0] = (uint8_t)abc[a >> 2];
        d[1] = (uint8_t)abc[(a & 3) << 4 | b >> 4];
        n += 2;
        if (rest == 2) d[2] = (uint8_t)abc[(b & 15) << 2], n++;
        if (!url) {
            if (rest == 1) d[2] = '=';
            d[3] = '=';
            n = 4 * g + 4;
        }
    }
    out->n += n;
}

/* Appends the bytes `text` encodes to `out`, or gives INVALID (22) and leaves `out` as it was:
 * for a character outside the alphabet, a length that can't be base64, padding where
 * there should be none (or, without `url`, none where there should be), or leftover bits
 * that aren't zero (RFC 4648 §3.5). */
static ZR_UNUSED uint8_t zr_b64_decode(zr_slice text, int32_t url, zr_vec *out) {
    const uint8_t *t = (const uint8_t *)text.p;
    int64_t len = text.n, n = len, pads = 0;
    while (n > 0 && t[n - 1] == '=' && pads < 2) n--, pads++;
    if ((!url || pads > 0) && len % 4 != 0) return 22;
    if (n % 4 == 1) return 22;
    int64_t g = n / 4, rest = n % 4;
    uint8_t *o = (uint8_t *)out->p + out->n;
    uint8_t seen;
#ifdef ZR_B64_SIMD
    int level = zr_simd_level();
    if (level == 2) seen = zr_b64_dec_avx512(o, t, g, url);
    else if (level == 1) seen = zr_b64_dec_avx2(o, t, g, url);
    else
#endif
        seen = zr_b64_dec_table(o, t, g, url);
    int64_t m = 3 * g;
    if (rest > 0) {
        const uint8_t *val = zr_b64_val[url], *q = t + 4 * g;
        uint8_t a = val[q[0]], b = val[q[1]], c = rest == 3 ? val[q[2]] : 0;
        seen |= a | b | c;
        if ((rest == 2 ? b & 15 : c & 3) != 0) return 22;
        o[3 * g] = (uint8_t)(a << 2 | b >> 4);
        if (rest == 3) o[3 * g + 1] = (uint8_t)(b << 4 | c >> 2);
        m += rest - 1;
    }
    if (seen == 255) return 22;
    out->n += m;
    return 0;
}

/* ------------------------------------------------------------ random (STDLIB §8.2) */

/* Bytes from the kernel. getrandom can only block briefly at early boot. */
static void zr_random_fill(void *p, size_t n) {
    char *c = (char *)p;
    while (n) {
        ssize_t r = getrandom(c, n, 0);
        if (r < 0) {
            if (errno == EINTR) continue;
            zr_fail("zink runtime", 0, "getrandom failed");
        }
        c += r;
        n -= (size_t)r;
    }
}

static ZR_UNUSED uint64_t zr_random_u64(void) {
    uint64_t v;
    zr_random_fill(&v, sizeof v);
    return v;
}

static ZR_UNUSED void zr_random_bytes(zr_vec *out, int64_t n) {
    if (n <= 0) return;
    zr_random_fill((char *)out->p + out->n, (size_t)n);
    out->n += n;
}

/* ------------------------------------------------------------ time (STDLIB §5.3) */

static ZR_UNUSED int64_t zr_time_now(void) {
    struct timespec t;
    clock_gettime(CLOCK_REALTIME, &t);
    return (int64_t)t.tv_sec * 1000000000 + t.tv_nsec;
}

static ZR_UNUSED int64_t zr_time_mono(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (int64_t)t.tv_sec * 1000000000 + t.tv_nsec;
}

/* Sleeps the whole duration, through signals; a negative one doesn't sleep. */
static ZR_UNUSED void zr_time_sleep(int64_t ns) {
    if (ns <= 0) return;
    struct timespec t = {ns / 1000000000, ns % 1000000000};
    while (nanosleep(&t, &t) < 0 && errno == EINTR) {}
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
/* MISSING for a slot freed since, INVALID for one the pool never had: the handle is from
 * another pool, or is the zero value a failed call gave. */
static ZR_UNUSED uint8_t zr_pool_check(zr_pool *p, zr_handle h, size_t sz) {
    if ((int64_t)h.i >= p->v.n) return 22;
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
/* `errno`'s value as a status, 0 when it's 0: EPERM is PERMISSION, since 1 is FAILED. */
static ZR_UNUSED uint8_t zr_errno_status(int e) {
    return (uint8_t)(e == EPERM ? EACCES : e >= 0 && e < 192 ? e : EIO);
}
static uint8_t zr_errno(void) { return errno > 0 ? zr_errno_status(errno) : (uint8_t)EIO; }

/* Per file descriptor: an input buffer, made on its first read and freed when it's closed,
 * and for a File or a Dir, a generation that each close bumps, so a copy of a closed one
 * can't reach whatever reuses its descriptor (DESIGN §6.4). A Dir keeps its DIR here. */
typedef struct { uint8_t *b; size_t pos, len; uint32_t gen; uint8_t open; DIR *dir; uint8_t *wb; size_t wn; } zr_inbuf;
static ZR_TLS zr_inbuf *zr_ins;
static ZR_TLS int zr_nins;

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
static uint8_t zr_write_fd(int fd, zr_slice s);

/* A File's writes are buffered, 64 KB at a time (STDLIB §4.1), and written when the buffer
 * fills, at flush, sync, close, before a read, a seek or asking its size or position, and
 * at exit. A failed write can show only then. */
#define ZR_WBUF ((size_t)1 << 16)

static uint8_t zr_wflush(int fd) {
    if (fd < 0 || fd >= zr_nins || !zr_ins[fd].wn) return 0;
    zr_inbuf *e = &zr_ins[fd];
    zr_slice s = {e->wb, (int64_t)e->wn};
    e->wn = 0;
    return zr_write_fd(fd, s);
}

static uint8_t zr_wput(int fd, const void *p, size_t n) {
    zr_inbuf *e = zr_in_entry(fd);
    if (e->wn + n > ZR_WBUF) {
        uint8_t s = zr_wflush(fd);
        if (s) return s;
    }
    if (n >= ZR_WBUF) {
        zr_slice d = {(void *)p, (int64_t)n};
        return zr_write_fd(fd, d);
    }
    if (!e->wb && !(e->wb = (uint8_t *)malloc(ZR_WBUF))) zr_fail("zink runtime", 0, "out of memory for a write buffer");
    memcpy(e->wb + e->wn, p, n);
    e->wn += n;
    return 0;
}

/* At exit: what open files still hold. */
static void zr_wflush_all(void) {
    for (int fd = 3; fd < zr_nins; fd++)
        if (zr_ins[fd].open && zr_ins[fd].wn) zr_wflush(fd);
}

static ZR_UNUSED uint8_t zr_io_close(int64_t h) {
    int fd = zr_io_fd(h);
    if (fd < 0) return 9;
    if (fd == 1) zr_flush();
    uint8_t w = zr_wflush(fd);
    zr_inbuf *e = &zr_ins[fd];
    free(e->b);
    free(e->wb);
    e->b = e->wb = NULL;
    e->pos = e->len = e->wn = 0;
    e->open = 0;
    e->gen++;
    uint8_t c = close(fd) < 0 && errno != EINTR ? zr_errno() : 0;
    return w ? w : c;
}

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
    return fd < 0 ? 9 : zr_wput(fd, s.p, (size_t)s.n);
}

/* Writes what stdout holds; nothing else is buffered. */
static ZR_UNUSED uint8_t zr_io_flush(int64_t h) {
    int fd = zr_io_fd(h);
    if (fd < 0) return 9;
    if (fd != 1) return zr_wflush(fd);
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
        ZR_OUT_LOCK();
        zr_out(fd, s.p, (size_t)s.n);
        zr_end_line(fd);
        ZR_OUT_UNLOCK();
        return 0;
    }
    uint8_t e = zr_wput(fd, s.p, (size_t)s.n);
    return e ? e : zr_wput(fd, "\n", 1);
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
    uint8_t w = zr_wflush(fd);
    if (w) { errno = w; return -1; }
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
    if (fd < 0) return 9;
    uint8_t w = zr_wflush(fd);
    return w ? w : zr_read_line(fd, v);
}

static ZR_UNUSED uint8_t zr_io_read_all(int64_t h, zr_vec *v) {
    int fd = zr_io_fd(h);
    if (fd < 0) return 9;
    uint8_t w = zr_wflush(fd);
    return w ? w : zr_read_all(fd, v);
}

/* About a File: 0 its size, 1 whether it's a terminal, 2 its position; -1 with errno set. */
static ZR_UNUSED int64_t zr_io_info(int64_t h, int32_t what) {
    int fd = zr_io_fd(h);
    if (fd < 0) { errno = EBADF; return -1; }
    uint8_t w = zr_wflush(fd);
    if (w) { errno = w; return -1; }
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
    uint8_t w = zr_wflush(fd);
    if (w) return w;
    zr_ins[fd].pos = zr_ins[fd].len = 0;
    return lseek(fd, (off_t)pos, SEEK_SET) < 0 ? zr_errno() : 0;
}

static ZR_UNUSED uint8_t zr_io_sync(int64_t h) {
    int fd = zr_io_fd(h);
    if (fd < 0) return 9;
    uint8_t w = zr_wflush(fd);
    if (w) return w;
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
            static ZR_TLS char buf[1 << 16];
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
    char tmpdir[4096];
    ZR_ENV_LOCK();
    const char *env = getenv("TMPDIR");
    snprintf(tmpdir, sizeof tmpdir, "%s", env && *env ? env : "/tmp");
    ZR_ENV_UNLOCK();
    const char *base = tmpdir;
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

/* The device and inode of what path names, following links, for fs.walk's loop check. */
static ZR_UNUSED uint8_t zr_fs_ident(zr_slice path, zr_slice out) {
    int64_t m = zr_sm();
    struct stat st;
    int r = stat(zr_cstr(path), &st);
    int e = errno;
    zr_sr(m);
    if (r < 0) { errno = e; return zr_errno(); }
    int64_t *o = (int64_t *)out.p;
    o[0] = (int64_t)st.st_dev;
    o[1] = (int64_t)st.st_ino;
    return 0;
}

static ZR_UNUSED uint8_t zr_fs_chdir(zr_slice path) {
    int64_t m = zr_sm();
    int r = chdir(zr_cstr(path));
    zr_sr(m);
    return r < 0 ? zr_errno() : 0;
}

/* ------------------------------------------------------------ processes (STDLIB §5.1, §5.2) */

static const char *zr_argv0 = "";

/* A pipe whose ends close on exec, so only the descriptors given to a child reach it. */
static int zr_pipe(int p[2]) {
    if (pipe(p) < 0) return -1;
    fcntl(p[0], F_SETFD, FD_CLOEXEC);
    fcntl(p[1], F_SETFD, FD_CLOEXEC);
    return 0;
}

/* A list of strings as NULL-terminated C strings, on the scratch stack. */
static char **zr_proc_strings(zr_slice list) {
    zr_slice *items = (zr_slice *)list.p;
    char **v = (char **)zr_scratch_alloc((size_t)(list.n + 1) * sizeof(char *));
    for (int64_t i = 0; i < list.n; i++) v[i] = (char *)zr_cstr(items[i]);
    v[list.n] = NULL;
    return v;
}

/* This environment with the changes applied: "NAME=VALUE" sets, a bare "NAME" unsets. */
static char **zr_proc_env(zr_slice changes) {
    char **ch = zr_proc_strings(changes);
    /* Another thread's env.set can move the array, though not the text it points at. */
    ZR_ENV_LOCK();
    int64_t n = 0;
    while (environ[n]) n++;
    char **v = (char **)zr_scratch_alloc((size_t)(n + changes.n + 1) * sizeof(char *));
    int64_t k = 0;
    for (int64_t i = 0; i < n; i++) {
        const char *e = environ[i];
        size_t len = strcspn(e, "=");
        int replaced = 0;
        for (int64_t j = 0; j < changes.n; j++) {
            size_t cl = strcspn(ch[j], "=");
            if (cl == len && memcmp(ch[j], e, len) == 0) { replaced = 1; break; }
        }
        if (!replaced) v[k++] = (char *)e;
    }
    ZR_ENV_UNLOCK();
    /* The last change to a name wins: a set that a later change undoes isn't kept. */
    for (int64_t j = 0; j < changes.n; j++) {
        if (!strchr(ch[j], '=')) continue;
        size_t jl = strcspn(ch[j], "=");
        int later = 0;
        for (int64_t l = j + 1; l < changes.n; l++)
            if (strcspn(ch[l], "=") == jl && memcmp(ch[l], ch[j], jl) == 0) { later = 1; break; }
        if (!later) v[k++] = ch[j];
    }
    v[k] = NULL;
    return v;
}

/* Starts argv[0], searched for in PATH unless it has a '/', with these descriptors as
 * its stdin, stdout and stderr (-1 to inherit). SIGPIPE goes back to its default, since
 * this program ignores it and an ignored signal stays ignored across exec. */
static int zr_proc_start(pid_t *pid, char **argv, char **envp, const char *cwd, int in, int out, int err) {
    posix_spawn_file_actions_t fa;
    posix_spawnattr_t at;
    posix_spawn_file_actions_init(&fa);
    posix_spawnattr_init(&at);
    if (in >= 0) posix_spawn_file_actions_adddup2(&fa, in, 0);
    if (out >= 0) posix_spawn_file_actions_adddup2(&fa, out, 1);
    if (err >= 0) posix_spawn_file_actions_adddup2(&fa, err, 2);
    if (cwd) posix_spawn_file_actions_addchdir_np(&fa, cwd);
    sigset_t def;
    sigemptyset(&def);
    sigaddset(&def, SIGPIPE);
    posix_spawnattr_setsigdefault(&at, &def);
    posix_spawnattr_setflags(&at, POSIX_SPAWN_SETSIGDEF);
    zr_flush();
    fflush(stderr);
    /* It reads the environment, and PATH from it, as it starts the child. */
    ZR_ENV_LOCK();
    int r = posix_spawnp(pid, argv[0], &fa, &at, argv, envp ? envp : environ);
    ZR_ENV_UNLOCK();
    posix_spawn_file_actions_destroy(&fa);
    posix_spawnattr_destroy(&at);
    return r;
}

static void zr_proc_exit(int ws, zr_slice result) {
    int64_t *r = (int64_t *)result.p;
    r[0] = WIFEXITED(ws) ? WEXITSTATUS(ws) : 0;
    r[1] = WIFSIGNALED(ws) ? WTERMSIG(ws) : 0;
}

/* Runs a program to its end: `input` fed to its stdin when `feed`, its stdout appended to
 * `out` and its stderr to `err` as `capture` says (bit 1 and bit 2), killed after `timeout`
 * nanoseconds when that's positive. `result` gets the exit code and signal. The status is
 * nonzero only when it couldn't be started, or for TIMEOUT. */
static ZR_UNUSED uint8_t zr_proc_run(zr_slice argv, zr_slice envs, zr_slice cwd, zr_slice input, int32_t feed,
                                     zr_vec *out, zr_vec *err, int32_t capture, int64_t timeout, zr_slice result) {
    int64_t m = zr_sm();
    char **av = zr_proc_strings(argv);
    char **ev = envs.n ? zr_proc_env(envs) : NULL;
    const char *dir = cwd.n ? zr_cstr(cwd) : NULL;
    int ip[2] = {-1, -1}, op[2] = {-1, -1}, ep[2] = {-1, -1};
    uint8_t st = 0;
    if ((feed && zr_pipe(ip) < 0) || ((capture & 1) && zr_pipe(op) < 0) || ((capture & 2) && zr_pipe(ep) < 0)) {
        st = zr_errno();
    }
    pid_t pid = 0;
    if (!st) {
        int r = zr_proc_start(&pid, av, ev, dir, ip[0], op[1], ep[1]);
        if (r) { errno = r; st = zr_errno(); }
    }
    int *child[3] = {&ip[0], &op[1], &ep[1]};
    for (int i = 0; i < 3; i++) if (*child[i] >= 0) { close(*child[i]); *child[i] = -1; }
    if (st) {
        if (ip[1] >= 0) close(ip[1]);
        if (op[0] >= 0) close(op[0]);
        if (ep[0] >= 0) close(ep[0]);
        zr_sr(m);
        return st;
    }
    if (ip[1] >= 0) {
        fcntl(ip[1], F_SETFL, fcntl(ip[1], F_GETFL) | O_NONBLOCK);
        if (input.n == 0) { close(ip[1]); ip[1] = -1; }
    }
    int64_t deadline = timeout > 0 ? zr_time_mono() + timeout : 0, fed = 0;
    while (ip[1] >= 0 || op[0] >= 0 || ep[0] >= 0) {
        struct pollfd pf[3];
        int k = 0, wait_ms = -1;
        if (ip[1] >= 0) pf[k++] = (struct pollfd){ip[1], POLLOUT, 0};
        if (op[0] >= 0) pf[k++] = (struct pollfd){op[0], POLLIN, 0};
        if (ep[0] >= 0) pf[k++] = (struct pollfd){ep[0], POLLIN, 0};
        if (deadline) {
            int64_t left = deadline - zr_time_mono();
            if (left <= 0) { st = 110; break; }
            wait_ms = (int)(left / 1000000) + 1;
        }
        int n = poll(pf, (nfds_t)k, wait_ms);
        if (n < 0 && errno == EINTR) continue;
        if (n < 0) { st = zr_errno(); break; }
        for (int i = 0; i < k; i++) {
            if (!pf[i].revents) continue;
            int fd = pf[i].fd;
            if (fd == ip[1]) {
                ssize_t w = write(fd, (const char *)input.p + fed, (size_t)(input.n - fed));
                if (w > 0) fed += w;
                if ((w < 0 && errno != EAGAIN && errno != EINTR) || fed == input.n) { close(fd); ip[1] = -1; }
            } else {
                zr_vec *v = fd == op[0] ? out : err;
                ssize_t r = read(fd, (char *)v->p + v->n, 1 << 16);
                if (r > 0) v->n += r;
                else if (r == 0 || (errno != EAGAIN && errno != EINTR)) {
                    close(fd);
                    if (fd == op[0]) op[0] = -1; else ep[0] = -1;
                }
            }
        }
    }
    if (ip[1] >= 0) close(ip[1]);
    if (op[0] >= 0) close(op[0]);
    if (ep[0] >= 0) close(ep[0]);
    int ws = 0;
    if (deadline && !st) {
        for (;;) {
            pid_t w = waitpid(pid, &ws, WNOHANG);
            if (w == pid) break;
            if (w < 0 && errno != EINTR) break;
            if (zr_time_mono() >= deadline) { st = 110; break; }
            zr_time_sleep(1000000);
        }
    }
    if (st == 110) kill(pid, SIGKILL);
    if (st == 110 || !deadline) while (waitpid(pid, &ws, 0) < 0 && errno == EINTR) {}
    zr_proc_exit(ws, result);
    zr_sr(m);
    return st;
}

/* Starts a program alongside this one, with this program's stdin, stdout and stderr. Its
 * pid, or -1 and errno. */
static ZR_UNUSED int64_t zr_proc_spawn(zr_slice argv, zr_slice envs, zr_slice cwd) {
    int64_t m = zr_sm();
    char **av = zr_proc_strings(argv);
    char **ev = envs.n ? zr_proc_env(envs) : NULL;
    pid_t pid;
    int r = zr_proc_start(&pid, av, ev, cwd.n ? zr_cstr(cwd) : NULL, -1, -1, -1);
    zr_sr(m);
    if (r) { errno = r; return -1; }
    return pid;
}

static ZR_UNUSED uint8_t zr_proc_wait(int64_t pid, zr_slice result) {
    int ws;
    pid_t w;
    do { w = waitpid((pid_t)pid, &ws, 0); } while (w < 0 && errno == EINTR);
    if (w < 0) return zr_errno();
    zr_proc_exit(ws, result);
    return 0;
}

static ZR_UNUSED uint8_t zr_proc_kill(int64_t pid, int32_t sig) {
    return kill((pid_t)pid, sig) < 0 ? zr_errno() : 0;
}

/* Replaces this process; returns only on failure. */
static ZR_UNUSED uint8_t zr_proc_exec(zr_slice argv) {
    char **av = zr_proc_strings(argv);
    zr_flush();
    fflush(stderr);
    signal(SIGPIPE, SIG_DFL);
    execvp(av[0], av);
    signal(SIGPIPE, SIG_IGN);
    return zr_errno();
}

static ZR_UNUSED int64_t zr_proc_pid(void) { return getpid(); }

static ZR_UNUSED zr_slice zr_proc_name(void) {
    zr_slice s = {(void *)zr_argv0, (int64_t)strlen(zr_argv0)};
    return s;
}

/* Appends the path PATH finds for `prog` (or `prog` itself when it has a '/') to out,
 * when that's an executable file; NOT_FOUND otherwise. */
static ZR_UNUSED uint8_t zr_proc_which(zr_slice prog, zr_vec *out) {
    int64_t m = zr_sm();
    const char *p = zr_cstr(prog);
    uint8_t st = 2;
    struct stat sb;
    if (prog.n == 0) {
    } else if (memchr(prog.p, '/', (size_t)prog.n)) {
        if (access(p, X_OK) == 0 && stat(p, &sb) == 0 && S_ISREG(sb.st_mode)) {
            memcpy((char *)out->p + out->n, p, (size_t)prog.n);
            out->n += prog.n;
            st = 0;
        }
    } else {
        ZR_ENV_LOCK();
        const char *env = getenv("PATH");
        if (!env) env = "/usr/local/bin:/usr/bin:/bin";
        size_t pl = strlen(env);
        char *path = (char *)zr_scratch_alloc(pl + 1);
        memcpy(path, env, pl + 1);
        ZR_ENV_UNLOCK();
        while (st && *path) {
            size_t dl = strcspn(path, ":");
            /* An empty entry is ".": dir, '/', the name and the NUL. */
            char *cand = (char *)zr_scratch_alloc((dl ? dl : 1) + (size_t)prog.n + 2);
            memcpy(cand, path, dl);
            size_t at = dl;
            if (dl == 0) cand[at++] = '.';
            cand[at++] = '/';
            memcpy(cand + at, prog.p, (size_t)prog.n);
            at += (size_t)prog.n;
            cand[at] = 0;
            if (access(cand, X_OK) == 0 && stat(cand, &sb) == 0 && S_ISREG(sb.st_mode)) {
                memcpy((char *)out->p + out->n, cand, at);
                out->n += (int64_t)at;
                st = 0;
            }
            path += dl;
            if (*path == ':') path++;
        }
    }
    zr_sr(m);
    return st;
}

/* (columns, rows) of stdout's terminal; NOT_TERMINAL. */
static ZR_UNUSED uint8_t zr_term_size(zr_slice result) {
    struct winsize w;
    if (ioctl(1, TIOCGWINSZ, &w) < 0) return 25;
    int64_t *r = (int64_t *)result.p;
    r[0] = w.ws_col;
    r[1] = w.ws_row;
    return 0;
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
    /* The last 0 to 7 bytes in two overlapping loads, or three single bytes, which cover
     * them all (the length was mixed in above). A memcpy of a variable n bytes into t
     * made a program counting words a third slower. */
    uint64_t t = 0;
    if (n >= 4) t = (uint64_t)zr_ld32(b) | (uint64_t)zr_ld32(b + n - 4) << 32;
    else if (n > 0) t = (uint64_t)b[0] | (uint64_t)b[n >> 1] << 8 | (uint64_t)b[n - 1] << 16;
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

#ifdef ZR_THREADS
/* ------------------------------------------------------------ threads (STDLIB §10.3) */

/* Workers by id, from 1: whether each has ended, and the status its function returned.
 * A record is kept for the whole run, 2 bytes, so a Worker can be joined any number of
 * times, from any thread. */
typedef struct { uint8_t done, status; } zr_wrec;
static pthread_mutex_t zr_wk_mu = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t zr_wk_cv = PTHREAD_COND_INITIALIZER;
static zr_wrec *zr_wk;
static int64_t zr_wk_n, zr_wk_cap;

typedef struct { uint8_t (*run)(void *); void *packet; int64_t id; } zr_wstart;

/* What a thread leaves when it ends: the Files it opened are written out and closed, since
 * no other thread can reach them, and its data stacks go back to the system. */
static void zr_thread_release(void) {
    for (int fd = 3; fd < zr_nins; fd++)
        if (zr_ins[fd].open) zr_io_close(((int64_t)zr_ins[fd].gen << 32) | (int64_t)fd);
    for (int fd = 0; fd < zr_nins; fd++) { free(zr_ins[fd].b); free(zr_ins[fd].wb); }
    free(zr_ins);
    zr_ins = NULL;
    zr_nins = 0;
    for (int i = 0; i < 4096; i++)
        if (zr_stacks[i]) { munmap(zr_stacks[i], ZR_STACK_BYTES); zr_stacks[i] = NULL; }
}

static void *zr_thread_main(void *arg) {
    zr_wstart w = *(zr_wstart *)arg;
    free(arg);
    /* stdin, stdout and stderr are Files in every thread (STDLIB §2.1). */
    for (int fd = 0; fd < 3; fd++) zr_in_entry(fd)->open = 1;
    uint8_t st = w.run(w.packet);
    free(w.packet);
    zr_thread_release();
    pthread_mutex_lock(&zr_wk_mu);
    zr_wk[w.id - 1].done = 1;
    zr_wk[w.id - 1].status = st;
    pthread_cond_broadcast(&zr_wk_cv);
    pthread_mutex_unlock(&zr_wk_mu);
    return NULL;
}

/* Starts `run` on a thread of its own with `packet`, which it frees: the worker's id, or
 * 0 with errno set when the system can't start one. */
static ZR_UNUSED int64_t zr_thread_spawn(uint8_t (*run)(void *), void *packet) {
    pthread_mutex_lock(&zr_wk_mu);
    if (zr_wk_n == zr_wk_cap) {
        int64_t cap = zr_wk_cap ? zr_wk_cap * 2 : 64;
        zr_wrec *t = (zr_wrec *)realloc(zr_wk, (size_t)cap * sizeof *t);
        if (!t) zr_fail("zink runtime", 0, "out of memory for a worker");
        zr_wk = t;
        zr_wk_cap = cap;
    }
    int64_t id = ++zr_wk_n;
    zr_wk[id - 1].done = 0;
    zr_wk[id - 1].status = 0;
    pthread_mutex_unlock(&zr_wk_mu);
    zr_wstart *w = (zr_wstart *)malloc(sizeof *w);
    if (!w) zr_fail("zink runtime", 0, "out of memory for a worker");
    w->run = run;
    w->packet = packet;
    w->id = id;
    pthread_attr_t a;
    pthread_attr_init(&a);
    pthread_attr_setdetachstate(&a, PTHREAD_CREATE_DETACHED);
    pthread_t t;
    int e = pthread_create(&t, &a, zr_thread_main, w);
    pthread_attr_destroy(&a);
    if (e) {
        free(w);
        free(packet);
        pthread_mutex_lock(&zr_wk_mu);
        zr_wk[id - 1].done = 1;
        zr_wk[id - 1].status = (uint8_t)e;
        pthread_mutex_unlock(&zr_wk_mu);
        errno = e;
        return 0;
    }
    return id;
}

/* Waits for a worker to end: the status its function returned, or INVALID for a Worker
 * spawn never gave (a failed spawn's zero value). */
static ZR_UNUSED uint8_t zr_thread_join(int64_t id) {
    pthread_mutex_lock(&zr_wk_mu);
    if (id <= 0 || id > zr_wk_n) {
        pthread_mutex_unlock(&zr_wk_mu);
        return 22;
    }
    while (!zr_wk[id - 1].done) pthread_cond_wait(&zr_wk_cv, &zr_wk_mu);
    uint8_t st = zr_wk[id - 1].status;
    pthread_mutex_unlock(&zr_wk_mu);
    return st;
}

static ZR_UNUSED int64_t zr_thread_cores(void) {
    long n = sysconf(_SC_NPROCESSORS_ONLN);
    return n > 0 ? (int64_t)n : 1;
}

/* ---- queues: values copied in by put and out by get, a node each, under a mutex, with a
 * condition variable for the threads waiting to get. A queue lasts for the whole run. A
 * queue that is a zero value, NULL, fails each call with INVALID. */
typedef struct zr_qnode { struct zr_qnode *next; max_align_t data[]; } zr_qnode;
typedef struct zr_queue {
    pthread_mutex_t m;
    pthread_cond_t c;
    zr_qnode *head, *tail;
    int64_t n, limit, size;
    int closed;
} zr_queue;

static ZR_UNUSED zr_queue *zr_q_new(int64_t size) {
    zr_queue *q = (zr_queue *)calloc(1, sizeof *q);
    if (!q) zr_fail("zink runtime", 0, "out of memory for a queue");
    pthread_mutex_init(&q->m, NULL);
    pthread_condattr_t ca;
    pthread_condattr_init(&ca);
    pthread_condattr_setclock(&ca, CLOCK_MONOTONIC);
    pthread_cond_init(&q->c, &ca);
    pthread_condattr_destroy(&ca);
    q->size = size;
    return q;
}

/* FULL at the queue's limit, END once it's closed; nothing is queued then. */
static ZR_UNUSED uint8_t zr_q_put(zr_queue *q, const void *v) {
    if (!q) return 22;
    zr_qnode *n = (zr_qnode *)malloc(sizeof *n + (size_t)q->size);
    if (!n) zr_fail("zink runtime", 0, "out of memory for a queued value");
    memcpy(n->data, v, (size_t)q->size);
    n->next = NULL;
    pthread_mutex_lock(&q->m);
    uint8_t st = q->closed ? 194 : q->limit > 0 && q->n >= q->limit ? 193 : 0;
    if (st == 0) {
        if (q->tail) q->tail->next = n; else q->head = n;
        q->tail = n;
        q->n++;
        pthread_cond_signal(&q->c);
    }
    pthread_mutex_unlock(&q->m);
    if (st) free(n);
    return st;
}

/* Takes the oldest value into `out`, waiting up to `ms` milliseconds for one (forever
 * when negative). EMPTY when there was none to take without waiting, TIMEOUT when the
 * wait ran out, END once the queue is closed and empty; `out` is zeroed then. */
static ZR_UNUSED uint8_t zr_q_get(zr_queue *q, void *out, int64_t ms) {
    if (!q) return 22;
    struct timespec until;
    if (ms > 0) {
        clock_gettime(CLOCK_MONOTONIC, &until);
        until.tv_sec += ms / 1000;
        until.tv_nsec += (ms % 1000) * 1000000;
        if (until.tv_nsec >= 1000000000) { until.tv_sec++; until.tv_nsec -= 1000000000; }
    }
    uint8_t st = 0;
    pthread_mutex_lock(&q->m);
    while (!q->head) {
        if (q->closed) { st = 194; break; }
        if (ms == 0) { st = 192; break; }
        if (ms < 0) {
            pthread_cond_wait(&q->c, &q->m);
        } else if (pthread_cond_timedwait(&q->c, &q->m, &until) == ETIMEDOUT && !q->head) {
            st = q->closed ? 194 : 110;
            break;
        }
    }
    zr_qnode *n = NULL;
    if (!st) {
        n = q->head;
        q->head = n->next;
        if (!q->head) q->tail = NULL;
        q->n--;
    }
    pthread_mutex_unlock(&q->m);
    if (n) {
        memcpy(out, n->data, (size_t)q->size);
        free(n);
    } else {
        memset(out, 0, (size_t)q->size);
    }
    return st;
}

/* No more puts: they fail with END, and gets do once what's queued has been taken. */
static ZR_UNUSED void zr_q_close(zr_queue *q) {
    if (!q) return;
    pthread_mutex_lock(&q->m);
    q->closed = 1;
    pthread_cond_broadcast(&q->c);
    pthread_mutex_unlock(&q->m);
}

/* At most n values queued, or no limit for 0; INVALID for a negative n. */
static ZR_UNUSED uint8_t zr_q_limit(zr_queue *q, int64_t n) {
    if (!q || n < 0) return 22;
    pthread_mutex_lock(&q->m);
    q->limit = n;
    pthread_mutex_unlock(&q->m);
    return 0;
}

static ZR_UNUSED int64_t zr_q_len(zr_queue *q) {
    if (!q) return 0;
    pthread_mutex_lock(&q->m);
    int64_t n = q->n;
    pthread_mutex_unlock(&q->m);
    return n;
}
#endif

/* The text status.name and status.message give for codes without one of their own: views
 * of these, which last the whole run in every thread, so they're filled at the start,
 * before any thread, from strerror, whose own buffer the next call or a thread's end
 * reuses. */
static char zr_num_tab[256][4];
static char zr_msg_tab[256][64];

static void zr_status_tables(void) {
    for (int s = 0; s < 256; s++) {
        snprintf(zr_num_tab[s], sizeof zr_num_tab[s], "%d", s);
        if (s < 192) snprintf(zr_msg_tab[s], sizeof zr_msg_tab[s], "%s", strerror(s));
        else snprintf(zr_msg_tab[s], sizeof zr_msg_tab[s], "Error %d", s);
    }
}

static ZR_UNUSED void zr_start(int argc, char **argv) {
    signal(SIGPIPE, SIG_IGN);
    zr_status_tables();
    /* glibc ignores the size unless it's given the buffer too. */
    static char obuf[1 << 16];
    if (!isatty(1)) {
        setvbuf(stdout, obuf, _IOFBF, sizeof obuf);
        zr_stdout = stdout;
    }
    zr_seed_init();
    zr_argv0 = argc > 0 ? argv[0] : "";
    /* stdin, stdout and stderr are Files from the start (STDLIB §2.1). */
    for (int fd = 0; fd < 3; fd++) zr_in_entry(fd)->open = 1;
    /* Files still open at exit are written out, however the program ends (DESIGN §6.4). */
    atexit(zr_wflush_all);
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

/* The names of DESIGN §7.7 and STDLIB §1.4; codegen::STATUS_NAMES is the same list. */
static const char *zr_status_name(uint8_t s) {
    switch (s) {
        case 0: return "OK";
        case 1: return "FAILED";
        case 2: return "NOT_FOUND";
        case 4: return "INTERRUPTED";
        case 5: return "IO";
        case 9: return "BAD_HANDLE";
        case 11: return "AGAIN";
        case 12: return "NO_MEMORY";
        case 13: return "PERMISSION";
        case 16: return "BUSY";
        case 17: return "EXISTS";
        case 18: return "CROSS_DEVICE";
        case 20: return "NOT_DIRECTORY";
        case 21: return "IS_DIRECTORY";
        case 22: return "INVALID";
        case 25: return "NOT_TERMINAL";
        case 28: return "NO_SPACE";
        case 32: return "BROKEN_PIPE";
        case 33: return "DOMAIN";
        case 34: return "RANGE";
        case 36: return "NAME_TOO_LONG";
        case 39: return "NOT_EMPTY";
        case 40: return "LOOP";
        case 95: return "UNSUPPORTED";
        case 110: return "TIMEOUT";
        case 192: return "EMPTY";
        case 193: return "FULL";
        case 194: return "END";
        case 195: return "MISSING";
        case 196: return "POSITION";
        case 197: return "TYPE";
        case 198: return "GPU";
        case 199: return "OUT_OF_DATE";
        case 200: return "ABANDONED";
        default: return zr_num_tab[s];
    }
}

static ZR_UNUSED zr_slice zr_status_message(uint8_t s) {
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
        case 200: m = "Abandoned: a failure inside wasn't read"; break;
        default: m = zr_msg_tab[s];
    }
    (void)zr_status_names;
    return zr_lit(m, (int64_t)strlen(m));
}

/* status.name (STDLIB §1.4): the name of DESIGN §7.7, the C name for an operating-system
 * code without one where the C library knows it, and "" for a program's own. */
extern const char *strerrorname_np(int) __attribute__((weak));
static ZR_UNUSED zr_slice zr_status_name_text(uint8_t s) {
    const char *n = zr_status_name(s);
    if (s >= 224) n = "";
    else if (n[0] >= '0' && n[0] <= '9' && s < 192 && strerrorname_np && strerrorname_np(s)) n = strerrorname_np(s);
    return zr_lit(n, (int64_t)strlen(n));
}
