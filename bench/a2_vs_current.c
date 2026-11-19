/* a2_vs_current.c: compares two memory designs for Zinkite.
 *
 * Current design (DESIGN.md §4-§8): strong references are counted; `owned`
 * fields are uncounted and freed with their owner; `owner` back-pointers are
 * plain uncounted pointers set by the runtime; `weak` references go through
 * the weak table (slot + generation); a borrow pins an object when the
 * compiler can't prove its field isn't written while the borrow lives.
 *
 * A.2 (DESIGN.md Appendix A.2): every reference field is strong and plain.
 * Each object keeps one entry per incoming reference (2 inline, then a heap
 * array). Locals that are live across a store are roots. When an object loses
 * a referrer or a root but still has referrers, a backward walk looks for a
 * rooted object; if there is none, everything the walk visited is garbage.
 * This is plain A.2, without the level/anchor optimization.
 *
 * Each workload is the C that each design's compiler and runtime would
 * produce for the same Zinkite program, written by hand. Both designs run the
 * same operations with the same random numbers, and their results are checked
 * against each other.
 *
 * Build: cc -O2 -o a2_vs_current a2_vs_current.c
 * Run:   ./a2_vs_current
 */
#define _POSIX_C_SOURCE 200809L
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

/* ================================================================ utilities */

static double now(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec + ts.tv_nsec * 1e-9;
}

static uint64_t rng_state;
static uint64_t rnd(void) {                       /* xorshift64* */
    rng_state ^= rng_state >> 12;
    rng_state ^= rng_state << 25;
    rng_state ^= rng_state >> 27;
    return rng_state * 0x2545F4914F6CDD1DULL;
}
static int rnd_below(int n) { return (int)(rnd() % (uint64_t)n); }

static void *xmalloc(size_t n) {
    void *p = malloc(n);
    if (!p) { perror("malloc"); exit(1); }
    return p;
}
static void *xcalloc(size_t n) {
    void *p = calloc(1, n);
    if (!p) { perror("calloc"); exit(1); }
    return p;
}
static void *xrealloc(void *p, size_t n) {
    p = realloc(p, n);
    if (!p) { perror("realloc"); exit(1); }
    return p;
}
static void check(int ok, const char *what) {
    if (!ok) { fprintf(stderr, "check failed: %s\n", what); exit(1); }
}

typedef struct { void **p; size_t n, cap; } Vec;
static void vpush(Vec *v, void *x) {
    if (v->n == v->cap) {
        v->cap = v->cap ? v->cap * 2 : 256;
        v->p = xrealloc(v->p, v->cap * sizeof *v->p);
    }
    v->p[v->n++] = x;
}

/* =========================================================== current design */

typedef struct { uint32_t classid, flags; uint64_t count; } Hdr;   /* §14.2 */

/* The weak table (§8.1). A weak reference is a slot plus the generation it
 * expects; freeing the object bumps the slot's generation. */
typedef struct { uint32_t slot, gen; } Weak;
static const Weak NO_WEAK = { 0, 0 };
static struct { void **target; uint32_t *gen, *freelist; uint32_t n, nfree, cap; } wt;

static void wt_init(void) {
    wt.cap = 1024;
    wt.n = 1;                                     /* slot 0: the null weak reference */
    wt.target = xcalloc(wt.cap * sizeof *wt.target);
    wt.gen = xcalloc(wt.cap * sizeof *wt.gen);
    wt.freelist = xcalloc(wt.cap * sizeof *wt.freelist);
    wt.gen[0] = 1;                                /* never matches NO_WEAK */
}
static uint32_t wt_alloc(void *obj) {
    uint32_t s;
    if (wt.nfree) {
        s = wt.freelist[--wt.nfree];
    } else {
        if (wt.n == wt.cap) {
            wt.cap *= 2;
            wt.target = xrealloc(wt.target, wt.cap * sizeof *wt.target);
            wt.gen = xrealloc(wt.gen, wt.cap * sizeof *wt.gen);
            wt.freelist = xrealloc(wt.freelist, wt.cap * sizeof *wt.freelist);
        }
        s = wt.n++;
        wt.gen[s] = 0;
    }
    wt.target[s] = obj;
    return s;
}
static void wt_release(uint32_t s) {
    wt.gen[s]++;
    wt.target[s] = NULL;
    wt.freelist[wt.nfree++] = s;
}
static inline void *weak_get(Weak w) {
    return wt.gen[w.slot] == w.gen ? wt.target[w.slot] : NULL;
}

static long c_live;

/* A doubly linked list:
 *   class Node { int value; owned Node? next; weak Node prev; }
 *   class List { owned Node? head; weak Node tail; } */
typedef struct CNode {
    Hdr h;
    uint32_t wslot;          /* DESIGN.md keeps this in a side map (§14.2) */
    int64_t value;
    struct CNode *next;      /* owned */
    Weak prev;               /* weak */
} CNode;
typedef struct { Hdr h; CNode *head; Weak tail; } CList;

static CNode *c_node_new(int64_t v) {
    CNode *n = xcalloc(sizeof *n);
    n->value = v;
    c_live++;
    return n;
}
static void c_node_free(CNode *n) {
    if (n->wslot) wt_release(n->wslot);
    free(n);
    c_live--;
}
static Weak c_weak_of(CNode *n) {
    if (!n->wslot) n->wslot = wt_alloc(n);
    return (Weak){ n->wslot, wt.gen[n->wslot] };
}

/* Freeing a node frees the rest of its owned chain (§4.3). */
static void c_free_chain(CNode *n) {
    while (n) {
        CNode *next = n->next;
        c_node_free(n);
        n = next;
    }
}

/* Node append(List l, int v) */
static CNode *c_list_append(CList *l, int64_t v) {
    CNode *n = c_node_new(v);                     /* fresh */
    CNode *t = weak_get(l->tail);                 /* a borrow from a weak read */
    if (t) {
        t->h.count++;                             /* pinned: Node.next is written below */
        n->prev = c_weak_of(t);
        t->next = n;                              /* owned store of a fresh object: no count */
        t->h.count--;
    } else {
        l->head = n;
    }
    l->tail = c_weak_of(n);
    return n;
}

/* void remove(List l, Node b) */
static void c_list_remove(CList *l, CNode *b) {
    b->h.count++;                                 /* the caller's pin on b (§7.1) */
    CNode *a = weak_get(b->prev);
    if (a) a->h.count++;                          /* pinned: Node.next is written below */
    CNode **slot = a ? &a->next : &l->head;
    CNode *gone = *slot;                          /* take(a.next): gone is b */
    *slot = NULL;
    CNode *after = gone->next;                    /* take(gone.next) */
    gone->next = NULL;
    if (after) after->prev = a ? c_weak_of(a) : NO_WEAK;
    else l->tail = a ? c_weak_of(a) : NO_WEAK;
    *slot = after;                                /* a detached object: no count */
    if (a) a->h.count--;
    if (--gone->h.count == 0) c_free_chain(gone); /* detached: freed when its last pin ends */
}

static uint64_t c_list_sum_fwd(CList *l) {        /* position-weighted: checks order */
    uint64_t s = 0, i = 0;
    for (CNode *n = l->head; n; n = n->next) s += (uint64_t)n->value * ++i;
    return s;
}
static uint64_t c_list_sum_back(CList *l) {
    uint64_t s = 0;
    for (CNode *n = weak_get(l->tail); n; n = weak_get(n->prev)) s += (uint64_t)n->value;
    return s;
}

/* A binary search tree with parent pointers:
 *   class Node { int key; owned Node? left; owned Node? right; owner Node parent; }
 * The root's owner is the Tree object; here its parent pointer is NULL. */
typedef struct CT { Hdr h; int64_t key; struct CT *left, *right, *parent; } CT;
typedef struct { Hdr h; CT *root; } CTree;

static CT *c_tree_insert(CTree *t, int64_t key) {
    CT *n = xcalloc(sizeof *n);
    n->key = key;
    c_live++;
    CT **slot = &t->root, *p = NULL;
    while (*slot) {
        p = *slot;
        slot = key < p->key ? &p->left : &p->right;
    }
    *slot = n;                                    /* owned store of a fresh object */
    n->parent = p;                                /* set by the runtime */
    return n;
}
static CT **c_slot_of(CTree *t, CT *x) {
    CT *p = x->parent;
    return !p ? &t->root : p->left == x ? &p->left : &p->right;
}
/* void rotateLeft(Node x): x's right child takes its place. */
static void c_rot_left(CTree *t, CT *x) {
    x->h.count++;                                 /* pinned: x's own slot is taken below */
    CT *p = x->parent;
    CT **slot = c_slot_of(t, x);
    CT *y = x->right; x->right = NULL;            /* take(x.right) */
    CT *b = y->left;  y->left = NULL;             /* take(y.left) */
    x->right = b; if (b) b->parent = x;
    *slot = NULL;                                 /* take(x's slot): x is detached */
    y->left = x;  x->parent = y;
    *slot = y;    y->parent = p;
    x->h.count--;
}
/* void rotateRight(Node y): y's left child takes its place. */
static void c_rot_right(CTree *t, CT *y) {
    y->h.count++;
    CT *p = y->parent;
    CT **slot = c_slot_of(t, y);
    CT *x = y->left;  y->left = NULL;
    CT *b = x->right; x->right = NULL;
    y->left = b; if (b) b->parent = y;
    *slot = NULL;
    x->right = y; y->parent = x;
    *slot = x;    x->parent = p;
    y->h.count--;
}
static uint64_t c_tree_shape(CT *n, uint64_t depth) {
    if (!n) return 0;
    return (uint64_t)n->key % 1000003 * depth
         + c_tree_shape(n->left, depth + 1) + c_tree_shape(n->right, depth + 1);
}
static void c_tree_free(CT *root) {               /* owned children go with their owner */
    Vec st = { 0 };
    if (root) vpush(&st, root);
    while (st.n) {
        CT *n = st.p[--st.n];
        if (n->left) vpush(&st, n->left);
        if (n->right) vpush(&st, n->right);
        free(n);
        c_live--;
    }
    free(st.p);
}

/* Shared objects held by ordinary strong fields:
 *   class Target { int v; }   class Holder { Target ref; } */
typedef struct { Hdr h; int64_t v; } CTarget;
typedef struct { Hdr h; CTarget *ref; } CHolder;

static void c_store(CHolder *h, CTarget *v) {     /* the write barrier of §4.4 */
    CTarget *old = h->ref;
    v->h.count++;
    h->ref = v;
    if (--old->h.count == 0) { free(old); c_live--; }
}

/* ====================================================================== A.2 */

typedef struct AObj AObj;
struct AObj {
    uint32_t classid;
    uint32_t roots;          /* references from globals and live locals */
    uint32_t mark;           /* walk epoch */
    uint16_t nref, cap;      /* one referrer entry per incoming reference */
    AObj **refs;             /* points at inl until there are more than 2 */
    AObj *inl[2];
    uint8_t dead;
    uint32_t nf;
    int64_t value;
    AObj *f[];               /* reference fields, all strong */
};

static long a_live;
static uint64_t a_steps;     /* objects visited by walks */
static uint32_t a_epoch;
static Vec a_check, a_stack, a_seen, a_dying;

static AObj *a_new(uint32_t nf, int64_t v) {
    AObj *o = xcalloc(sizeof(AObj) + nf * sizeof(AObj *));
    o->refs = o->inl;
    o->cap = 2;
    o->nf = nf;
    o->value = v;
    a_live++;
    return o;
}
static void ref_add(AObj *t, AObj *src) {
    if (t->nref == t->cap) {
        check(t->cap < 32768, "referrer list too long");
        uint16_t cap = (uint16_t)(t->cap * 2);
        AObj **r = xmalloc(cap * sizeof *r);
        memcpy(r, t->refs, t->nref * sizeof *r);
        if (t->refs != t->inl) free(t->refs);
        t->refs = r;
        t->cap = cap;
    }
    t->refs[t->nref++] = src;
}
static void ref_remove(AObj *t, AObj *src) {
    for (uint32_t i = 0; i < t->nref; i++) {
        if (t->refs[i] != src) continue;
        t->refs[i] = t->refs[--t->nref];
        if (t->refs != t->inl && t->nref <= 2) {  /* back to inline storage */
            memcpy(t->inl, t->refs, t->nref * sizeof *t->refs);
            free(t->refs);
            t->refs = t->inl;
            t->cap = 2;
        }
        return;
    }
    check(0, "referrer missing");
}

/* Does a rooted object reach o? Follows referrers backwards, depth first,
 * and leaves every visited object in a_seen. Each object's oldest referrer
 * is tried first, since it is usually the one on the path from the root;
 * trying the newest first sends a walk down into sibling subtrees. */
static int a_walk(AObj *o) {
    a_epoch++;
    a_stack.n = a_seen.n = 0;
    o->mark = a_epoch;
    vpush(&a_stack, o);
    vpush(&a_seen, o);
    while (a_stack.n) {
        AObj *x = a_stack.p[--a_stack.n];
        a_steps++;
        if (x->roots) return 1;
        for (uint32_t i = x->nref; i-- > 0;) {
            AObj *r = x->refs[i];
            if (r->mark == a_epoch) continue;
            r->mark = a_epoch;
            vpush(&a_stack, r);
            vpush(&a_seen, r);
        }
    }
    return 0;
}

/* Frees a set of objects that nothing rooted reaches. Their outgoing
 * references are dropped first, which can leave other objects to check. */
static void a_kill(void **set, size_t n) {
    for (size_t i = 0; i < n; i++) ((AObj *)set[i])->dead = 1;
    for (size_t i = 0; i < n; i++) {
        AObj *x = set[i];
        for (uint32_t k = 0; k < x->nf; k++) {
            AObj *t = x->f[k];
            if (t && !t->dead) { ref_remove(t, x); vpush(&a_check, t); }
        }
        vpush(&a_dying, x);
    }
}
static void a_process(void) {
    while (a_check.n) {
        AObj *o = a_check.p[--a_check.n];
        if (o->dead || o->roots) continue;
        if (o->nref == 0) {
            void *one = o;
            a_kill(&one, 1);
        } else if (!a_walk(o)) {
            a_kill(a_seen.p, a_seen.n);
        }
    }
    for (size_t i = 0; i < a_dying.n; i++) {     /* memory goes back only after all checks */
        AObj *x = a_dying.p[i];
        if (x->refs != x->inl) free(x->refs);
        free(x);
        a_live--;
    }
    a_dying.n = 0;
}

static void a_store(AObj *src, uint32_t k, AObj *v) {   /* src.f[k] = v */
    AObj *old = src->f[k];
    if (v) ref_add(v, src);                       /* add before remove */
    src->f[k] = v;
    if (old) {
        ref_remove(old, src);
        vpush(&a_check, old);
        a_process();
    }
}
static void a_root(AObj *o) { o->roots++; }
static void a_unroot(AObj *o) {
    if (--o->roots == 0) {
        vpush(&a_check, o);
        a_process();
    }
}

/* The same list with every reference strong:
 *   class Node { int value; Node? next; Node? prev; }
 *   class List { Node? head; Node? tail; } */
enum { HEAD = 0, TAIL = 1 };
enum { NEXT = 0, PREV = 1 };

static AObj *a_list_append(AObj *l, int64_t v) {
    AObj *n = a_new(2, v);
    a_root(n);                                    /* local n */
    AObj *t = l->f[TAIL];
    if (t) {
        a_root(t);                                /* local t, live across stores */
        a_store(n, PREV, t);
        a_store(t, NEXT, n);
    } else {
        a_store(l, HEAD, n);
    }
    a_store(l, TAIL, n);
    if (t) a_unroot(t);
    a_unroot(n);
    return n;
}

static void a_list_remove(AObj *l, AObj *b) {
    a_root(b);                                    /* parameter b */
    AObj *a = b->f[PREV], *c = b->f[NEXT];
    if (a) a_root(a);
    if (c) a_root(c);
    if (a) a_store(a, NEXT, c); else a_store(l, HEAD, c);
    if (c) a_store(c, PREV, a); else a_store(l, TAIL, a);
    if (c) a_unroot(c);                           /* locals released in reverse order */
    if (a) a_unroot(a);
    a_unroot(b);
}

static uint64_t a_list_sum_fwd(AObj *l) {
    uint64_t s = 0, i = 0;
    for (AObj *n = l->f[HEAD]; n; n = n->f[NEXT]) s += (uint64_t)n->value * ++i;
    return s;
}
static uint64_t a_list_sum_back(AObj *l) {
    uint64_t s = 0;
    for (AObj *n = l->f[TAIL]; n; n = n->f[PREV]) s += (uint64_t)n->value;
    return s;
}

/* The same tree with every reference strong:
 *   class Node { int key; Node? left; Node? right; Node? parent; } */
enum { LEFT = 0, RIGHT = 1, PARENT = 2 };
enum { ROOT = 0 };

static AObj *a_tree_insert(AObj *t, int64_t key) {
    AObj *n = a_new(3, key);
    a_root(n);
    AObj *p = NULL, *cur = t->f[ROOT];
    while (cur) {
        p = cur;
        cur = cur->f[key < cur->value ? LEFT : RIGHT];
    }
    if (!p) {
        a_store(t, ROOT, n);
    } else {
        a_root(p);
        a_store(p, key < p->value ? LEFT : RIGHT, n);
        a_store(n, PARENT, p);
        a_unroot(p);
    }
    a_unroot(n);
    return n;
}
static void a_rot_left(AObj *t, AObj *x) {
    a_root(x);
    AObj *p = x->f[PARENT], *y = x->f[RIGHT], *b = y->f[LEFT];
    if (p) a_root(p);
    a_root(y);
    if (b) a_root(b);
    a_store(x, RIGHT, b);
    if (b) a_store(b, PARENT, x);
    a_store(y, PARENT, p);
    if (!p) a_store(t, ROOT, y);
    else a_store(p, p->f[LEFT] == x ? LEFT : RIGHT, y);
    a_store(y, LEFT, x);
    a_store(x, PARENT, y);
    if (b) a_unroot(b);
    a_unroot(y);
    if (p) a_unroot(p);
    a_unroot(x);
}
static void a_rot_right(AObj *t, AObj *y) {
    a_root(y);
    AObj *p = y->f[PARENT], *x = y->f[LEFT], *b = x->f[RIGHT];
    if (p) a_root(p);
    a_root(x);
    if (b) a_root(b);
    a_store(y, LEFT, b);
    if (b) a_store(b, PARENT, y);
    a_store(x, PARENT, p);
    if (!p) a_store(t, ROOT, x);
    else a_store(p, p->f[LEFT] == y ? LEFT : RIGHT, x);
    a_store(x, RIGHT, y);
    a_store(y, PARENT, x);
    if (b) a_unroot(b);
    a_unroot(x);
    if (p) a_unroot(p);
    a_unroot(y);
}
static uint64_t a_tree_shape(AObj *n, uint64_t depth) {
    if (!n) return 0;
    return (uint64_t)n->value % 1000003 * depth
         + a_tree_shape(n->f[LEFT], depth + 1) + a_tree_shape(n->f[RIGHT], depth + 1);
}

/* ================================================================ workloads */

static void row(const char *name, double tc, double ta, long ops, uint64_t steps) {
    double nc = tc / ops * 1e9, na = ta / ops * 1e9;
    printf("%-46s %9.1f %10.1f %8.1fx %11.1f\n", name, nc, na, na / nc, (double)steps / ops);
}

/* Holders pointing at shared targets: one field store per operation. A keeper
 * object holds every target, so no target is ever freed. */
static void w_shared(int m, int k, int r) {
    CTarget **ct = xmalloc(k * sizeof *ct);
    CHolder **ch = xmalloc(m * sizeof *ch);
    for (int i = 0; i < k; i++) {
        ct[i] = xcalloc(sizeof **ct);
        ct[i]->v = i;
        ct[i]->h.count = 1;                       /* the keeper's reference */
        c_live++;
    }
    for (int i = 0; i < m; i++) {
        ch[i] = xcalloc(sizeof **ch);
        ch[i]->ref = ct[i % k];
        ct[i % k]->h.count++;
        c_live++;
    }
    rng_state = 99;
    double t0 = now();
    for (int i = 0; i < r; i++) {
        int h = rnd_below(m), t = rnd_below(k);
        c_store(ch[h], ct[t]);
    }
    double tc = now() - t0;
    uint64_t csum = 0;
    for (int i = 0; i < m; i++) csum += (uint64_t)ch[i]->ref->v;

    AObj *keeper = a_new((uint32_t)k, 0);
    a_root(keeper);
    AObj **at = xmalloc(k * sizeof *at);
    AObj **ah = xmalloc(m * sizeof *ah);
    for (int i = 0; i < k; i++) {
        at[i] = a_new(0, i);
        a_store(keeper, (uint32_t)i, at[i]);
    }
    for (int i = 0; i < m; i++) {
        ah[i] = a_new(1, 0);
        a_root(ah[i]);                            /* held by a global array */
        a_store(ah[i], 0, at[i % k]);
    }
    rng_state = 99;
    uint64_t s0 = a_steps;
    t0 = now();
    for (int i = 0; i < r; i++) {
        int h = rnd_below(m), t = rnd_below(k);
        a_store(ah[h], 0, at[t]);
    }
    double ta = now() - t0;
    uint64_t asum = 0;
    for (int i = 0; i < m; i++) asum += (uint64_t)ah[i]->f[0]->value;
    check(csum == asum, "shared: results differ");

    char name[80];
    snprintf(name, sizeof name, "strong field store (%d holders, %d targets)", m, k);
    row(name, tc, ta, r, a_steps - s0);

    for (int i = 0; i < m; i++) {
        CTarget *t = ch[i]->ref;
        if (--t->h.count == 0) { free(t); c_live--; }
        free(ch[i]);
        c_live--;
    }
    for (int i = 0; i < k; i++)
        if (--ct[i]->h.count == 0) { free(ct[i]); c_live--; }
    for (int i = 0; i < m; i++) a_unroot(ah[i]);
    a_unroot(keeper);
    check(c_live == 0 && a_live == 0, "shared: objects leaked");
    free(ct); free(ch); free(at); free(ah);
}

/* Traversals: forward uses owned/strong `next` in both designs; backward
 * uses `weak prev` in the current design and a strong `prev` in A.2. */
static void w_list_walk(int n, int reps) {
    CList *cl = xcalloc(sizeof *cl);
    AObj *al = a_new(2, 0);
    a_root(al);
    for (int i = 0; i < n; i++) {
        c_list_append(cl, i);
        a_list_append(al, i);
    }
    volatile uint64_t sink = 0;
    double t0 = now();
    for (int i = 0; i < reps; i++) sink += c_list_sum_fwd(cl);
    double tcf = now() - t0;
    t0 = now();
    for (int i = 0; i < reps; i++) sink += a_list_sum_fwd(al);
    double taf = now() - t0;
    t0 = now();
    for (int i = 0; i < reps; i++) sink += c_list_sum_back(cl);
    double tcb = now() - t0;
    t0 = now();
    for (int i = 0; i < reps; i++) sink += a_list_sum_back(al);
    double tab = now() - t0;
    check(c_list_sum_back(cl) == a_list_sum_back(al), "walk: results differ");
    (void)sink;

    row("list: read next (owned vs strong)", tcf, taf, (long)n * reps, 0);
    row("list: read prev (weak vs strong)", tcb, tab, (long)n * reps, 0);

    c_free_chain(cl->head);
    free(cl);
    a_unroot(al);
    check(c_live == 0 && a_live == 0, "walk: objects leaked");
}

/* Remove a random node from the middle, append a new one at the end. */
static void w_list_churn(int n, int r) {
    CList *cl = xcalloc(sizeof *cl);
    CNode **cn = xmalloc(n * sizeof *cn);
    for (int i = 0; i < n; i++) cn[i] = c_list_append(cl, i);
    rng_state = 42;
    double t0 = now();
    for (int i = 0; i < r; i++) {
        int j = rnd_below(n);
        c_list_remove(cl, cn[j]);
        cn[j] = c_list_append(cl, n + i);
    }
    double tc = now() - t0;

    AObj *al = a_new(2, 0);
    a_root(al);
    AObj **an = xmalloc(n * sizeof *an);
    for (int i = 0; i < n; i++) an[i] = a_list_append(al, i);
    rng_state = 42;
    uint64_t s0 = a_steps;
    t0 = now();
    for (int i = 0; i < r; i++) {
        int j = rnd_below(n);
        a_list_remove(al, an[j]);
        an[j] = a_list_append(al, n + i);
    }
    double ta = now() - t0;
    check(c_list_sum_fwd(cl) == a_list_sum_fwd(al), "churn: forward results differ");
    check(c_list_sum_back(cl) == a_list_sum_back(al), "churn: backward results differ");

    char name[80];
    snprintf(name, sizeof name, "list: remove + append (%d nodes)", n);
    row(name, tc, ta, r, a_steps - s0);

    c_free_chain(cl->head);
    free(cl); free(cn);
    a_unroot(al);
    free(an);
    check(c_live == 0 && a_live == 0, "churn: objects leaked");
}

/* Rotate at a node and rotate back, so the tree keeps its shape. */
static void w_tree_rotate(int n, int r, int at_root) {
    int64_t *keys = xmalloc(n * sizeof *keys);
    rng_state = 7;
    for (int i = 0; i < n; i++) keys[i] = (int64_t)(rnd() >> 1);

    CTree *ct = xcalloc(sizeof *ct);
    CT **cn = xmalloc(n * sizeof *cn);
    for (int i = 0; i < n; i++) cn[i] = c_tree_insert(ct, keys[i]);
    uint64_t shape = c_tree_shape(ct->root, 1);
    rng_state = 11;
    long rotations = 0;
    double t0 = now();
    for (int i = 0; i < r; i++) {
        CT *x = at_root ? ct->root : cn[rnd_below(n)];
        if (x->right) { c_rot_left(ct, x); c_rot_right(ct, x->parent); rotations += 2; }
        else if (x->left) { c_rot_right(ct, x); c_rot_left(ct, x->parent); rotations += 2; }
    }
    double tc = now() - t0;
    check(c_tree_shape(ct->root, 1) == shape, "tree: current design changed shape");

    AObj *at = a_new(1, 0);
    a_root(at);
    AObj **an = xmalloc(n * sizeof *an);
    for (int i = 0; i < n; i++) an[i] = a_tree_insert(at, keys[i]);
    check(a_tree_shape(at->f[ROOT], 1) == shape, "tree: shapes differ after building");
    rng_state = 11;
    uint64_t s0 = a_steps;
    t0 = now();
    for (int i = 0; i < r; i++) {
        AObj *x = at_root ? at->f[ROOT] : an[rnd_below(n)];
        if (x->f[RIGHT]) { a_rot_left(at, x); a_rot_right(at, x->f[PARENT]); }
        else if (x->f[LEFT]) { a_rot_right(at, x); a_rot_left(at, x->f[PARENT]); }
    }
    double ta = now() - t0;
    check(a_tree_shape(at->f[ROOT], 1) == shape, "tree: A.2 changed shape");

    char name[80];
    snprintf(name, sizeof name, "tree: rotation at %s (%d nodes)",
             at_root ? "the root" : "a random node", n);
    row(name, tc, ta, rotations, a_steps - s0);

    c_tree_free(ct->root);
    free(ct); free(cn);
    a_unroot(at);
    free(an); free(keys);
    check(c_live == 0 && a_live == 0, "tree: objects leaked");
}

int main(void) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    wt_init();
    printf("%-46s %9s %10s %9s %11s\n", "nanoseconds per operation", "current", "A.2",
           "A.2/cur", "walk steps");
    w_shared(100000, 25000, 5000000);
    w_list_walk(100000, 100);
    w_list_churn(1000, 20000);
    w_list_churn(10000, 20000);
    w_list_churn(100000, 2000);
    w_tree_rotate(100000, 200000, 0);
    w_tree_rotate(100000, 200, 1);
    printf("\nbytes per list node: current %zu + %zu in the weak table; A.2 %zu, plus a heap\n"
           "array while a node has more than 2 referrers\n",
           sizeof(CNode), sizeof(void *) + sizeof(uint32_t), sizeof(AObj) + 2 * sizeof(AObj *));
    printf("all results matched between the two designs, and nothing leaked\n");
    return 0;
}
