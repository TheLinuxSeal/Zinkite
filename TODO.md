# TODO: memory-safety audit, 2026-11-25

Four audits wrote programs that `zink check` accepts and that then read or write memory
they shouldn't: reference lifetimes, removals and moved elements, the generated C, and the
C runtime. Each item below was reproduced by running it; the repros are minimal versions.
The numbers are the ones used when the audit was reported.

Items 1–27 are fixed (2026-11-25), each with a regression test: 1–21 in
`compiler/tests/lifetimes.rs`, 22–27 in `compiler/tests/run.rs`. The repros below are
what those tests check.

A dangling reference into a data stack is invisible to AddressSanitizer (a data stack is
one mapping), so most repros print a wrong value instead: `777` or `overwritten-BBB` where
the program should not have compiled.

Already known and not repeated here: stack overflow has no message, there are no guard
pages, and `#max_memory_mb` isn't enforced (docs/15-not-yet.md).

## Aliasing across calls (decided 2026-11-25: through summaries)

- [x] **1. Aliasing across a call (rule R).** Inside a function, two `&` parameters, a
  parameter and `this`, or a parameter and a static are treated as separate storage, so a
  reference through one survives a removal through the other.
  ```
  void f(i64[..~]& a, i64[..~]& b) { i64& r = a[0]; b.clear(); b +> 777; println("r=${r}"); }
  void main() { i64[..~] x = []; x +> 5; f(x, x); }                       // r=777
  ```
  Also `f(a, a[0])` where `f` pops `a` and reads its `i32&` parameter; a method given
  `this.xs[0]`; `zap(m, m.getOrAdd(1, 0))`; `b.merge(b)`; a static cleared while a
  parameter refers into it; `r.bump(a)` (method syntax) accepted where `bump(r, a)` is
  refused, since the receiver is evaluated before `user_call_with` collects `uses`
  (lifetimes.rs ~3315, ~3474). Summaries (`summarize`, ~892) record which formals a
  function removes from, not that it keeps using another formal's references after.
  DESIGN §5.2 says there are no aliasing rules. Fixed with summaries: they record "uses a
  reference into X after removing from Y", and a call where X and Y share a root is an
  error (REFERENCES §4.2); a receiver given to a free function in method syntax is one of
  its arguments' uses.

## Checker: rule R (removals and moves)

- [x] **2. Calls through `func` values have no effects**, and their results are rooted only
  at their arguments (lifetimes.rs:3365, `Origin::Value` emits no `Ev::Call`; REFERENCES
  §4.3 says such a call uses the union of every stored function).
  ```
  func(i32[..~]& => void) g = (i32[..~]& b) => { @_ = b ->; };
  i32& x = a[0]; g(a); a +> 777; println("x=${x}");                       // x=777
  ```
  Also through a `func` field and a `func` parameter. Rule S side:
  ```
  void apply(string[..>]& out, func(i64 => string) f) { out +> f(0); }
  apply(keep, (i64 k) => { string v = buf; return v[k:15]; });           // keep[0] dangles
  ```
  - **Lambdas passed to built-ins are checked once, straight through** (lifetimes.rs
    ~2800–2845): no loop for the per-element calls, no holder, no call event.
    ```
    i64[..~]& ar = a;   // a = [30, 10, 20]
    a.map((i64 x) => { if (x == 30) { ar.clear(); ar +> 777; } return x; });   // [777]
    a.count((i64 v) => { ar.clear(); ar +> 777; println("v=${v}"); return true; }); // v=777
    ```
    `retain` brings removed elements back; a capture used on a later pass after a removal
    on an earlier one is missed.
- [x] **3. Interface dispatch picks the implementation's summary by name and parameter
  count** (lifetimes.rs:3447), so an overload gets the wrong one. Codegen dispatches right.
  ```
  interface Z { void zap(i32[..~]& a); }
  class K implements Z { i64 q; void zap(i64 a) {} void zap(i32[..~]& a) { a.clear(); } }
  Z& z = ks.push(new K(1)); i32& x = a[0]; z.zap(a); a +> 777; println("x=${x}");   // 777
  ```
  The same for rule S: `void put(i64 x) {} void put(string s) { saved = s; }`.
- [x] **4. `defer` is checked in declaration order**, though defers run last-first
  (lifetimes.rs:2674, `d.extend(nodes)`), **and `break`/`continue` skip the defers of the
  blocks they leave** (`live()`, 1661–1666).
  ```
  i32& x = a[0]; defer println("x=${x}"); defer a +> 777; defer a.clear();    // x=777
  while (true) { defer println("x=${x}"); a.clear(); a +> 777; break; }       // x=777
  ```
- [x] **5. Whole-value assignment isn't always a removal.**
  - Through a reference (lifetimes.rs:3106–3118, `=` through `T&` only stores):
    `i64[..~]& ar = a; i64& r = a[0]; ar = []; a +> 777;` → r=777; the same through a `&`
    parameter and for capped arrays; a variable-size value rebuilt through `Order&` or
    `u8[.]&` leaves `string name = o.customer` stale.
  - A class value holding a capped array that can remove (lifetimes.rs:3120–3130,
    3146–3155 count only Array/Map/Set and variable-size locals):
    `class Bag { i32[..~8] xs; } i32& r = b.xs[0]; b = new Bag([]); b.xs +> 777;` → 777;
    also `bags[0] = ...`, a `Bag&`, `reset(Bag&)`, `bags.swap(0, 1)`.
- [x] **6. A constructor built in place can remove from or grow its own destination**
  (codegen/records.rs:486 `calls_out` assumes it can't; the checker doesn't look).
  ```
  class Node { i64 key; Node&<i32>? next; new(Node[..>]& xs, i64 k) { xs.clear(); key = k; next = null; } }
  Node& a = tree.push(new Node(tree, 1)); tree.push(new Node(other, 777)); println(a.key); // 777
  ```
  With variable-size records a constructor that clears and refills `recs` while built into
  it leaves a record decoding as 0x4141414141414141; one that only grows it corrupts too.
- [x] **7. Rings and gap buffers: moves that aren't recorded** (lifetimes.rs:3614–3627;
  `builtin()` calls `convert()` lazily, so an argument it never converts never records
  that the buffer becomes one block).
  - On a wrapped `i64[~.~4]`: `partialSort`, `radixSort`, `radixSortBy`, `selectNth`,
    `partialSortBy`, `selectNthBy`, `sortByDescending` move `r = q[0]`; `sort()` is refused.
  - `u8[~8] g = "abcd"; g.seek(1); g +^ 'X'; u8& r = g[3]; g.append("zz");` → r 'c' → 'z'.
  - `println(g)`, `o.append(g)`, `g.split("b")` make it one block unrecorded; a later
    `r = 'Z'` is lost.
  - Suspected, not runnable yet: `heapPush`, `heapify`, `appendUtf8` on gap buffers.
- [x] **8. Per-layout copies (DESIGN §4.3) are checked as the declared `T[..>]&`**, which
  never moves elements (`moves(rt)`, lifetimes.rs:3614), though a ring or gap buffer can.
  Fixed by refusing a ring or gap buffer for such a parameter; checking each copy for its
  own layout would allow it again.
  - `void add(i64[..>]& xs, i64 v) { xs +> v; }` with a gap buffer: r 30 → 40.
  - `void srt(i64[..>]& xs) { xs.sort(); }` with a wrapped ring: r 2 → 3.
  - `void look(i64[..>]& xs) { i64& a = xs[0]; i64[:] s = xs[0:xs.len]; println("${a}"); }`
    with a wrapped ring prints another element.
- [x] **9. The pool exemption covers arrays stored in pools** (lifetimes.rs:1760,
  `exempt_ty(root_ty)`): `pool<i64[..~8]> p; i64[..~8]& arr = p.get(h); i64& r = arr[0];
  arr.clear(); arr +> 777;` → 777; a `pool<i64[~8]>` element's moves go unchecked.
- [x] **10. Summaries drop stores from a parameter, or `this`, into another part of the
  same object** (the `a.var != d.var` filter in `summarize`, lifetimes.rs:948, 999).
  ```
  root Lexer { u8[..>] text; string[..>] words; }
  void add(Lexer& lx, string s) { i64 st = lx.text.len; lx.text.append(s); lx.words +> lx.text[st:]; }
  add(lx, "secret-AAAAAAAA"); lx.text.clear(); add(lx, "overwritten-BBB"); println(lx.words[0]);
  ```
  Also methods (`mut void reset() { text.clear(); }`) and a plain class with `u8[..>16]`.

## Checker: rule S (references outliving their storage)

The repros use this frame, where a dangling view prints `overwritten-BBB`:
```
for (i64 round = 0; round < 2; round++) {
    u8[..>] buf = [];
    buf.append(round == 0 ? "secret-AAAAAAAA" : "overwritten-BBB");
    if (round == 0) { /* ESCAPE */ }
}
println(/* the escaped view */);
```

- [x] **11. Destructuring into existing names** (lifetimes.rs:2656, `Target::Existing`): a
  static or field target records no store (`name_var` gives None):
  `(saved, n) = pair(buf);`; a `T&` local or parameter target records a store into the
  reference's own cell, though codegen writes through it (`(h, n) = mk(buf);`), and
  `summarize` skips cell stores, so callers never see a parameter stored into.
- [x] **12. Lambda parameters have empty root sets** (lifetimes.rs:2814, only `Ev::Def`):
  `parts.count((string s) => { saved = s; return true; })`;
  `hs.count((Holder& h) => { h.s = buf; return true; })`;
  `func(string => void) f = (string s) => remember(s); f(buf);`.
- [x] **13. Fixpoint loops stop at a fixed count and then accept**: `root_sets`
  (lifetimes.rs:833, 64 rounds; a chain of 65 assignments gets through) and `solve`
  (1035, 100 rounds; a chain of 100 functions each returning the next one's result).
  Iterate to convergence, or report an error.
- [x] **14. `this` escapes a constructor** (`map_formal`'s `Var::This`, ~693, maps to
  nothing without a receiver; the constructor's summary drops `This`, ~1022; codegen builds
  the object in a C temporary).
  ```
  class Node { i64 v; Node&? me = null; new(i64 v) { this.v = v; me .= this; } }
  Node n = new Node(111); Node&? m = n.me; n.v = 222;
  if (m != null) { println("${m.v}"); }                  // 00 at -O2; ASan stack-use-after-scope
  ```
  Also `new(Reg& r, i64 v) { ...; r.last .= this; }`.
- [x] **15. For-each over a user iterator gives the loop variable the roots of the iterator
  expression, not of `next()`** (lifetimes.rs:2479; the hidden iterator local is no root).
  ```
  class Chars { u8[..~16] data; i64 at = 0;
    mut string next() { if (at + 4 > data.len) { return #status.END ""; } at++; return data[at-1:at+3]; } }
  for (string s : new Chars("secret-AAAAAAAA")) { if (keep.len == 0) { keep +> s; } }
  u8 c = keep[0][0];                                       // ASan stack-use-after-scope
  ```
  Also a class with `iterator()`, `for (string s : b)` with `b` local to the loop.
- [x] **16. `?:` and `switch` arms that convert an array to a view lose its root**
  (lifetimes.rs:2871 keeps a cell only when both arms are places; 2715 and the yields keep
  only `.refs`).
  ```
  string f(string t, bool p) { u8[..>] b = []; b.append(t); return p ? b : "literal"; }
  string s = f("secret-AAAAAAAA", true); string t = f("overwritten-BBB", false);
  println("${s} ${t}");                                    // overwritten-BBB literal
  ```
  Also `saved = round == 0 ? buf : "literal";`, `case 0 => buf;`, `yield buf;`.
- [x] **17. Field initializers aren't checked** (`build()` puts none into a constructor's
  flow; a default constructor's body is empty, ~575).
  ```
  class Box { string label = "box-${Box.count}-AAAAAAAAAA"; static i64 count = 7; }
  Box make(string tag) { return new Box(); }
  Box b = make("tag ${1}"); show("overwritten ${2}-BBBBBBBBBBBBBBBB"); println(b.label); // ritten 2-BBB...
  ```
  Suspected too: static initializers aren't analyzed at all (`i64 X = keep("...".trim());`
  storing into a static works only because `zr_init_statics` never resets the scratch
  stack).
- [x] **18. `holds_refs` is cached wrong for recursive classes** (lifetimes.rs:436): a class
  reached while another is being evaluated is cached as holding no references.
  `class A { B b; string s; } class B { A[.] xs; }`, then
  `A[..>] tmp = []; tmp +> new A(new B([]), buf); saved = new B(tmp);` → a dangling
  `a.s`; with A's fields swapped it is refused.
- [x] **19. A worker can reach a static holding views through the calls a for-each makes**
  (`find_spawns`/`worker_statics`, ~1132–1240, follow only `E::Call`/`E::New`, not the
  implicit `next()`/`iterator()`). A worker running `for (string s : new It())`, whose
  `next()` returns the static view `first = buffer[0:15]`, read the main thread's
  `buffer` as main changed it. Calling `it.next()` directly is refused.

## Checker: layout rules (V and L)

- [x] **20. Variable-size values assigned through a reference overwrite what follows**
  (V1 is checked only for member targets, check.rs:3932–3945; `vs_assign`,
  codegen/records.rs ~806, memmoves any size; `store_into`'s `on_stack` branch, codegen.rs
  ~2547, for frozen arrays).
  ```
  class Item { u8[.*] sku; i32 qty; }
  items +> new Item("a", 1); items +> new Item("b", 2);
  for (Item& it : items) { it = new Item("much-longer-sku-xxxxxxxxxxxxxxxx", 5); break; }
  for (Item& it : items) { println("${it.sku} ${it.qty}"); }           // second is " 0"
  ```
  A shorter record gives the next one a length of 144115188075855871. Also through
  `void set(Item& it) { it = ...; }`, `Item& i = b.it; i = ...`, and
  `for (i64[.]& r : rows) { r = [7,7,7,7,7,7]; }` over `i64[.][..>]`.
- [x] **21. Values with self-relative links copied out of their array** (L2; the copy check
  is only in `expect`, check.rs:1395, and `moves_elements`, check.rs:5450/7277, lists
  methods only). Following the copied link segfaults.
  - `Node[..>] part = tree[1:];` and `tree[0:1] = tree[1:2];`
  - `tree.fill(tree[2])`
  - `hashmap<i64, Node>.put(1, b)` and `getOrAdd`; `pool<Node>.add(b)`
  - `Node[:]& w = tree[0:1]; w = tree[1:2];`

## Generated code

- [x] **22. `reverse()` on an empty capped array with a cap above 65535 segfaults**: the
  counter is `uint32_t` (`counter()`, codegen.rs:7311), `Arr::len()` returns `a->n`
  uncast (7357), so `_j = a->n - 1` (6369) wraps to 4294967295.
  `u8[..~70000] c = []; c.reverse();`. The ring version `u8[~.~70000]` loops 2^31 times.
- [x] **23. Chunked arrays (`$`) after `clear()`**: `zr_chk_clear` (zink_rt.h ~667) resets
  `L`/`M` but keeps `heads[]` from the larger chunk size, and `zr_chk_take_at` (~612)
  uses an unmasked head, so two positions share a slot.
  ```
  i64[~$] c = []; for (i64 i = 0; i < 2000; i++) { c +> i; }
  for (i64 i = 0; i < 20; i++) { @_ = c -<; } c.clear();
  for (i64 i = 0; i < 40; i++) { c +> 1000 + i; } @_ = c -<; println("${c}");  // ...1032, 1017, ..., 52, ...
  ```
- [ ] **128-bit indexes are cut to 64 bits before the check** (wrong element, not out of
  bounds): `a[(u128)1 << 64]` gives `a[0]`; the same for slice bounds, `getOr`, `seek`
  (`index_slot`, codegen.rs ~4211; `slice_bound`, ~4240).
- [ ] Suspected: misaligned pointers (UB) for compound assignment on a `packed` field
  (`int64_t *_p = &(field)`) and frozen arrays inside variable-size records
  (`((int64_t*)p)[i]`); UBSan reports them, output is right with clang and gcc today.
- [ ] Not memory bugs, but C that doesn't compile: `it.sku.len` on a `u8[.*]` field of a
  loop variable, `x.nums.fill(7)` on a variable-size record, `for (u8 c : r.name)`
  through a reference. `m = []` through a `hashmap&` parameter compiles to `(void)0`.

## Runtime

- [x] **24. `x.fixed(d)` reads past its buffer**: `char b[400]` (zink_rt.h:917–923), but
  `1.7976931348623157e308.fixed(100)` is 410 characters, and `zr_put` writes `n`.
- [x] **25. `status.name` and `status.message` return views of reused buffers**: `static
  ZR_TLS char buf[8]`/`buf[32]` (zink_rt.h ~3206, 3242, 3247) and `strerror()`'s buffer
  (~3260), which glibc frees on the next call and at thread exit.
  `string a = status.name(201); string b = status.name(202);` prints `a` as 202;
  `string e1 = status.message(150); string e2 = status.message(151); println(e1);` is a
  heap-use-after-free, and a static set from one in a worker dangles after it ends.
- [x] **26. `environ` races with threads**: `zr_env_set` takes `ZR_ENV_LOCK`, but
  `zr_proc_env` (zink_rt.h ~2607–2621), `posix_spawnp(..., environ)` (~2655) and the
  `getenv` calls (~2461, ~2812) don't. A worker calling `env.set` while another runs a
  `Command` with `setEnv` is a heap-use-after-free.
- [x] **27. Deques and cursor arrays used as queues drift through their reservation**:
  `zr_deq_settle` (zink_rt.h:388) recenters only when empty, as do `zr_cur_push` /
  `zr_cur_take_front` (466–478). `i64[~.~] q = []; q +> 0;` then `q +> i; q -<;` in a loop
  moves 8 bytes per operation: 783 MB after 100M operations, and past the end of the 8 GB
  stack, into the next mapping, after about 537M.
- [x] `zr_proc_which` wrote n+3 bytes into n+2 for an empty PATH entry (fixed with 26).
- [ ] Suspected: a hashmap's entries reaching 4 GB overwrite its index (one 8 GB
  reservation split in half); cursor slot numbers are `uint32_t` with `UINT32_MAX` as
  "none", so 2^32 inserts wrap.

## Security, not memory corruption

- [ ] NUL bytes are truncated wherever text crosses into C (`zr_cstr`, zink_rt.h:358):
  `File.readAll("real.txt\0.png")` reads `real.txt`; the same for `writeAll`, `mkdir`,
  `env.set`, `process.run` arguments; `process.which("sh\0x")` returns `/usr/bin/sh\0x`.
  Should be `INVALID`.
- [ ] `Command` arguments can set the child's environment: `Command.arg`
  (lib/zink/os/Command.zn:22–25, `split` 95–99) stores items NUL-separated, and one
  starting with byte 1 or 2 reads back as an env set or unset.
  `c.arg("\u{1}INJECTED=x")` sets `INJECTED`; `c.arg("x\0y")` is two arguments.
- [ ] Zero-value handles alias live ones: a failed spawn's `Child.of(0)` (Command.zn:73)
  makes `kill()` call `kill(0, SIGTERM)`, the whole process group, and `wait()` reap any
  child; a zero `File` is handle 0, stdin (`zr_io_fd` ~2021, `zr_start` ~3181), so
  `close()` closes stdin; started with fd 0 closed, the first File shares stdin's handle
  and its writes are lost at exit (`zr_wflush_all` starts at fd 3, ~2139).
- [ ] `File.writeAll`'s temporary file is created 0666 & umask and given the old mode only
  after the write (zink_rt.h:2322–2325): a 0600 file's new contents are world-readable
  meanwhile.
- [ ] Suspected: `fs.removeAll` opens directories by path after `readdir`, so a directory
  swapped for a symlink is followed (the time-of-check race other languages' remove-all
  had).

## Wrong results

- [ ] CRC-32's lazy table (`if (!table[1])`, zink_rt.h:1356) races when threads first use
  it together: 5 wrong CRCs in 1000 runs of 8 workers.
- [ ] A worker that stops the program loses the main thread's buffered File writes: the
  atexit flush walks the exiting thread's TLS list.
- [ ] `fs.copy(a, a)`, or through a link to itself, truncates the file and returns OK
  (`O_TRUNC` before the read, zink_rt.h:2417).
- [ ] `File.update`: a write after `readLine` lands at the read-ahead offset (`zr_wput`,
  2121, doesn't drop the input buffer or seek back).
- [ ] `f64.parse` rejects text of 128 bytes or more (`char buf[128]`, zink_rt.h:1225), so
  JSON `asF64` of a 401-digit integer is INVALID and `asF64Or` 0.0; it accepts `0x1p3`,
  `infinity` and `nan(123)`.
- [ ] `Value.asU64` of "-99999999999999999999" is 0 with OK (lib/zink/data/Value.zn:188
  takes RANGE's 0 for "-0").
- [ ] A `Command` timeout near the largest `Duration` overflows `zr_time_mono() + timeout`
  (zink_rt.h:2700) and kills the child at once with TIMEOUT.
- [ ] stdin is buffered per thread: after main reads one line, a worker gets END though
  lines are left.
- [ ] Smaller: `JsonWriter` writes invalid UTF-8 raw and says OK; `CsvWriter` doesn't quote
  a leading U+FEFF, so it's lost on reading back; a `fs.walk` left by `break` leaks its
  directory descriptors; a worker ending with a `Dir` open leaks the `DIR`
  (`zr_thread_release` uses `zr_io_close`); `toTimestamp()` of 9999-12-31 stops on
  overflow even under `@` (DateTime.zn:177); `iso()` of hand-built extreme fields stops
  (DateTime.zn:144).
- [ ] Some correct programs are refused: a whole-array `T[..~]&` field, and a static
  pool-link array that refers to itself.
