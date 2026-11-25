# What the example programs showed

> Four programs written against `DESIGN.md`, `GRAMMAR.md` v2 and `REFERENCES.md` as of 2026-11-17:
> - `csvsort.zn`: sort CSV rows by a column;
> - `wordfreq.zn`: the most common words on stdin;
> - `linebuf.zn`: an editor line buffer driven by commands;
> - `dirwalk.zn`: total sizes per directory, like `du`.
>
> There is no compiler yet, so they are checked by hand against the grammar and the rules, not compiled.
>
> Section 5 covers a second batch of four, written on 2026-11-19. Sections 6 and 7 cover
> two larger programs, written and run on 2026-11-24 with the compiler, and section 8 the
> first program with threads.

---

## 1. What worked

- **Character literals as integers.** `line[i] == ','`, `paths +> '/'` and `b >= 'a'` needed no casts (DESIGN §9.1).
- **The status in conditions and loops** reads well:
  - `if (@stdin.readLine(line)) { break; }`
  - `Pending p @empty = todo ->; if (empty) { break; }`. This pops until the stack is empty with no separate length check.
  - Both are now repeat loops (DESIGN §9.4): `while (Pending p : todo ->)` and `while (string line : stdin.nextLine(input))`. The examples use them.
- **The zero value on failure is useful.** In `csvsort`, a key that isn't a number parses as `0.0` and simply sorts first. No special case was needed (DESIGN §7.5).
- **Root summaries matched intuition.** `field(line, col)` and `firstWord(s)` return views rooted at their argument, and `d.next(names)` returns an entry whose name is rooted at `names`. Nothing had to be annotated.
- **Views into one big input buffer** (`text` in `csvsort` and `wordfreq`, `paths` in `dirwalk`) can be stored in arrays and maps, because the buffer only grows (REFERENCES §5.3).
- **Rule R stayed out of the way.** A reused line buffer, cleared at the top of each iteration, works because the views into it are declared inside the loop body.
- **`defer` inside a loop** closes each directory at the end of its iteration, and a `continue` before the `defer` correctly skips it.
- **`switch` on strings**, with block arms where needed, fit argument parsing and command dispatch.

---

## 2. Decisions needed

All ten were decided on 2026-11-18; each item says where.

Ordered by how often they came up.

1. **Constructors for plain data classes.** Every data class (`Row`, `Options`, `Count`, `Pending`, `DirTotal`) needed a hand-written constructor that assigns each field in turn. DESIGN.md's own examples (`new Order(1001, "alice", ...)`) already assumed one existed.
   Decided: a class that declares no constructor gets one taking every field without an initializer, in declaration order (DESIGN §6.2). The examples now use it.
2. **Exit status.** Nothing in the design set it, so the examples used `return #1;` from `main`. Decided: `u8 main(...)` returns the status, `void main` exits with 0, and `exit(code)` ends the program from anywhere (DESIGN §8.6). The examples now return `1`.
3. **Writing to stderr.** Decided: `errprint` and `errprintln`, shorthand for `stderr.print` and `stderr.println` (DESIGN §4.13).
4. **The type of integer literals.** Every loop is written `for (i64 i = 0; ...)`, because lengths and indexes are `i64` while `var i = 0` would have been an `i32`. Decided: with nothing else to decide it, an integer literal is an `i64`, so `var i = 0` is an `i64` (DESIGN §9.1).
5. **Ignoring a bit on purpose.** `f64 parsed @notNumber = f64.parse(key);` declared a variable only to stop the dropped-bit warning, and the variable was then never read. Decided: `_` can be assigned anything and never read, so this is now `num @_ = f64.parse(key);` (DESIGN §7.3).
6. **Arguments a call changes are invisible at the call site.** `File.readAll(path, text)`, `stdin.readLine(line)` and `d.next(names)` all grow their last argument, and only the signature (`u8[..>]&`) says so.
   Decided: no marker, as in Goose. Writing `&text` at the call means the same and draws a warning (DESIGN §5.1).
7. **Maps and the one-growable-field rule.** Decided: a built-in `hashmap<K, V>`, which the compiler gives two data stacks, like a cursor array (DESIGN §4.14). The original problem: a hash map needs a growable table, and rehashing builds a new table while the old one is still live. With one growable field per class (REFERENCES §8.2), the standard library's `map` must either copy every entry twice when it grows or be allowed two growable fields.
   Root structs are no longer needed for maps. They stay open for user classes that need several growable fields (REFERENCES §12).
8. **For-each over library types.** Decided: for-each works over iterators, values with a `mut T next()` method that return a nonzero status at the end, so `codePoints()`, `lines()` and `splitBy()` are lazy (DESIGN §9.4). The original question: `counts` (a map), `s.codePoints()` and `text[:].split('\n')` were all iterated with for-each.
   **Options**: they return arrays, built on a data stack, or they use an iteration interface for classes (then Open). A lazy `codePoints()` and `split()` would avoid building an array just to loop over it.
9. **Two results from one function.** Decided: tuple results, `(string, string) splitFirst(string s)`, destructured with `(string cmd, string arg) = splitFirst(line);` (DESIGN §9.5). `linebuf` now uses them. The original problem: `linebuf` scans each line twice, once in `firstWord` and once in `afterFirstWord`, because a function returns one value plus a status. A two-field class works, and it would be easy with decision 1.
10. **Where interpolated temporaries live.** `out.append("${text[i].utf8()}")` passes freshly built text to a `string` parameter.
    Decided (DESIGN §4.11): outputs and `append` get the parts written directly, so this example needs no temporary at all. Variables get the text built in place. Anything else gets an exact-size temporary on a scratch data stack, released at the end of the statement. Keeping a view of a temporary is a compile error that suggests `u8[.]`.

---

## 3. Smaller notes

- **The recursion rule shaped `dirwalk`.** It uses an explicit stack of pending directories instead of recursion (REFERENCES §8.4).
  - The cost is a little more code.
  - The benefit is that very deep trees can't overflow the native stack.
- **`dirwalk` keeps every path for the whole run**, since `paths` only grows. For a CLI tool that's fine (about the total length of all paths). A long-running program would want a pool, which is DESIGN §3.4's known trade-off.
- **Cursor arrays can't be sliced**, so `linebuf`'s `show` encodes one code point at a time. A library method such as `out.appendUtf8(text, from, to)` would encode a range in one call.
- **String ordering** used `a.compareTo(b) < 0`. Decided: `string` is `ordered`, so `<` works on it directly (DESIGN §10.1), and the examples now use `a.key < b.key`.
- **`list[:top]` stops the program when `top > list.len`.** `wordfreq` uses a loop with two conditions instead. A clamped slice bound such as `list[:|top]`, matching the `[|i]` index mode, would say it directly. (Slices are now written `a[i:j]`, not `a[i..j]`; DESIGN §4.10.)
- **`u8` arithmetic** in `lower()` (`b + 32`) is checked for overflow even though the range test above it rules overflow out. The check is one branch.

---

## 4. Standard library surface the examples assume

This is what the four programs needed. `STDLIB.md` now designs all of it, and more.

| Area | Used |
|---|---|
| Output | `println`, `errprintln`, `print` |
| Input | `stdin.readAll(u8[..>]&)`, `stdin.readLine(u8[..>]&)`, now `stdin.nextLine(u8[..~]&)` |
| Files | `File.readAll(string, u8[..>]&)`, `File.open`, `File.create`, `close` |
| Directories | `Dir.open(string)`, `d.next(u8[..>]&)` returning a `DirEntry { string name; bool isDir; i64 size; }`, `d.close()` |
| Text | `split(string)`, `lines()`, `splitBy(string)`, `codePoints()` (the last three are iterators), `compareTo`, `==`, `append(string)`, slices |
| Numbers | `i64.parse`, `f64.parse`, `pad(w)`, `utf8()` |
| Arrays | `push`/`+>`, pop/`->`, `clear`, `sort(lambda)`, `partialSort`, `reverse`, `len` |
| Cursor arrays | `+^`, `-^`, `^-`, `move`, `seekStart`, `seekFromEnd`, `cursor` |
| Maps | `hashmap<K, V>` (built in): `getOrAdd(key, initial)` returning `V&`, for-each over entries with `key` and `value` |

---

## 5. The second batch

> Four more programs, written against the design as of 2026-11-19 to exercise what the first batch couldn't: roots, pools and links, writable views, the repeat loop, overloading, bounds and the sorts. They were checked by hand and by a reviewer that hadn't seen the design discussion.
> - `cachesim.zn`: replay a trace of keys through an LRU cache and report the hit rate;
> - `sortlines.zn`: a small `sort(1)`, with `-r`, `-n`, `-V`, `-u`, `--header` and `--top`;
> - `stats.zn`: count, min, max, mean, standard deviation and percentiles, and with `-i` the most common values;
> - `jsonfmt.zn`: re-indent JSON, streaming by default, with sorted keys under `-s`.

### 5.1 What worked
- **Roots.** The LRU cache is a generic root holding a pool and a map (DESIGN §6.6). Its nodes form a doubly linked list through `T&?` references into the pool, rebound with `.=` and cleared with `null`.
- **Writable views.** `lines[first:]` is sorted, reversed and assigned through while the header line stays put. `pending[start:].stableSortBy(...)` sorts one object's children inside an array shared by every object.
- **The repeat loop with `@why`** reads lines and JSON events, and tells a clean end of input from an error after the loop.
- **Bounds.** `summarize` and `quantile` are written once over `T implements number` and used with both `i64` and `f64`.
- **Overloading.** `w.write(...)` picks its overload by the argument's type: a string, a `bool`, a number.
- **Typed lambdas** read clearly in every sort call, and the sort family covered every ordering the programs needed: `radixSortBy`, `stableSort` with `compareNatural`, `partialSort`, `selectNth`.
- **Statuses.** Every call that can fail is handled, or discarded with `@_`, and `return #(why)` passes one on.
- **No reachable overflow**, even with defaults such as `--top` = `i64.MAX`, which only ever reaches `min`.
- **Rule R by liveness.** `lines.dedup()` comes after the writable view's last use, and `stack ->` after the last use of a reference into `stack`.

### 5.2 Decisions
All eleven were decided on 2026-11-19; each item says where.

1. **Rule R works per part of a root.** `cachesim` removes a key from `cache.index` while its list references point into `cache.nodes`. With roots recorded only as `cache`, that looked like a removal under live references. A reference into a part is now rooted at the part, and the pool exemption applies to a pool part (REFERENCES §8.2, DESIGN §6.6).
2. **The rule against undecided features covers language features only**: DESIGN §17 and GRAMMAR §8, not STDLIB §11's questions about the library's design, which would otherwise have forbidden `stableSort` and the JSON packages. And `stdin`, `stdout` and `stderr` are `File` handles, so `File input = stdin;` works (STDLIB §2.1).
3. **`pool`, `hashmap` and `hashset` fields start empty**, like array fields, and aren't parameters of a default constructor (DESIGN §6.2).
4. **A local or parameter hides a class with the same name**, as in Java (DESIGN §8.5). Every program had a local named `path`, which `import zink.io.*` also brings in as a class. They are named `file` now anyway, for clarity.
5. **Narrowing follows the flow** for locals and parameters: `if (last == null) { return; }` narrows `last` from there on. Fields aren't narrowed, since a call could change them, so `cachesim` copies `n.prev` into a local before testing it (DESIGN §5.1).
6. **A reference field's first assignment in a constructor binds it**; after that `=` writes through it (DESIGN §6.2).
7. **`return #why;`** works with a `u8` or `bool` variable, not only `#(why)` (DESIGN §7.2).
8. **Writers can copy a number's text.** `jsonfmt` had to round-trip numbers through `i64` or `f64`, losing digits and rewriting `1.50`. Now `writeNumber(text)` writes them as read, and `write(Value)` keeps a number's text (STDLIB §9.7).
9. **A float out of range parses as ±infinity, with no status**, as the float casts do (STDLIB §2.4). `RANGE` stays for integers.
10. **Generic type arguments are inferred from the arguments.** An array passed where a slice is expected gives its element type, so `summarize(ints)` is `T = i64` (DESIGN §10).
11. **`Handle<T>`** is typed by the pool's element type, so a handle can't be passed to a pool of another type (STDLIB §7.5).

Also clarified: generic code may cast a `T` to any number type (DESIGN §10.1), and an array's slots govern only adding and removing, so a `T[..>]&` parameter may be sorted (DESIGN §4.3). Smaller fixes: REFERENCES §9 still said a map uses two data stacks, and `Handle`, `JsonEvent` and `JsonToken` were missing from the package lists.

### 5.3 Bugs the review found in the programs, now fixed
- **`sortlines -n -u` left duplicates.** The stable sort by number kept lines with equal numbers in input order, so equal lines weren't adjacent for `dedup`. The lines are now sorted by text first.
- **`sortlines -r -n` reversed lines with equal numbers.** `-r` now negates the key instead of reversing afterwards.
- **`sortlines -n` read the whole line as a number**, where `sort -n` reads the number a line starts with. It uses `parsePrefix` now.
- **`stats` printed the lower middle value as the median** of an even count. `quantile` now interpolates between the two nearest values.
- **`jsonfmt`'s comments** claimed it handled input of any size, and gave the wrong reason for avoiding recursion: it's the native stack's depth.
- **Three programs printed an empty name for stdin** in their read errors.
- **The first batch never imported `zink.io`**, though `csvsort` and `dirwalk` use `File` and `Dir`, which `zink.lang` doesn't bring in.

### 5.4 Smaller notes
- **There's no conditional `defer`.** `cachesim` closes its file only if it opened one, so the close is written at the end, and an early `return` relies on the backstop at exit (DESIGN §6.4).
- **`i64.MAX` as a default is a trap** when overflow stops the program: `first + top` would stop it, so `sortlines` uses `min(top, lines.len)`. A clamped slice bound such as `lines[:|top]` (§3) would say it directly.
- **`stats` computes everything in `f64`**, so integers beyond 2^53 print rounded. Exact integer statistics would need `i128` or `zink.bigint`.
- **An interpolated quantile** needs `selectNth` and then the minimum of what follows it. A library `quantile` would save every program from writing that.


---

## 6. quarry: a game through C's headers

> `quarry/quarry.zn`, about 1,800 lines: a voxel sandbox drawn with OpenGL through SDL2,
> both reached by `extern import` (DESIGN §12.6). It was written against the compiler,
> and checked by running it: frames drawn with SDL's offscreen driver and looked at, with
> clicks injected into a copy.

### 6.1 What worked
- **Calling C as it is.** 41 SDL and OpenGL functions, and constants such as
  `GL_DEPTH_TEST` and `SDL_SCANCODE_W`, needed nothing written for them.
- **Explicit declarations for pointers.** 11 functions needed an explicit `extern`
  declaration, each one line saying what its pointer means:
  - vertex data, pixels and fog colors are slices of numbers;
  - buffer ids, screenshots and the event are writable views C fills.

  An `SDL_Event` is an array of 16 `u32`s whose fields are read by their offsets.
- **Wrapping arithmetic** made the noise's hash plain code: `h * 0x94D049BB133111EB`,
  with `(u64%) x` to reinterpret a coordinate.
- **Statuses.** `aim` fails with `MISSING` when nothing is in reach, which `Hit aimed
  @nothing = aim(...)` reads. Loading a world checks nothing step by step: a short file
  fails where it reads past the end, and `@@` in `main` catches it.
- **Roots.** The world, with its blocks and its column heights, and the mesh builder,
  with a vertex array for cubes and one for water, are roots with two growable parts.
  Each is passed by reference everywhere, with nothing to annotate.
- **It runs as fast as C would.**
  - At `-O2`, generating 5 million blocks and building 256 chunk meshes take about 0.3 s.
  - A frame costs about 0.1 ms of the CPU.
  - At `-O0`, as `zink run` builds by default, it starts in about a second.

### 6.2 Fixed in the compiler while writing it
- **A typedef of an unnamed enum**, as `typedef enum { ... } SDL_bool;`, wasn't read as an
  enum, so `SDL_GL_SetAttribute` and `SDL_SetRelativeMouseMode` couldn't be called. SDL
  declares most of its types this way. clang gives such a typedef's type as its own
  name, and the reader now keeps the written type, `enum SDL_bool`, for it.
- **The cache of what clang found in a header** was kept across versions of the reader.
  Its key now includes the reader's version, so a fixed reader isn't hidden by an old
  entry.
- **`split`** type-checked but had no code generated for it. It's now written in Zinkite,
  in `zink.lang`'s `TextMethods`, over `splitBy`.
- **An `f32` printed as the double it widens to**, `0.10000000149011612`. It now prints
  as the shortest text that reads back as the same `f32`, `0.1`.

### 6.3 What it ran into
- **No constants.** Block ids are `u8` statics, `u8 GRASS = 1;`, which are variables, so
  they can't be `case` labels. Since 2026-11-25, `final u8 GRASS = 1;` is a constant
  (DESIGN §6.1), which can be one, and quarry's block ids and sizes are `final`.
- **No integer for an enum constant.** An enum can't index an array or be written to a
  file as a byte. So a block's id is a `u8`, and how it's drawn is an enum, `Kind`, in a
  table indexed by the id.
- **A `char **` can't cross into C**, so shaders can't be compiled, and the drawing is
  OpenGL's fixed pipeline. A `void *` result, as `SDL_GLContext`, can only be declared
  as an integer.
- **`extern import` takes no defines.** GL 1.5's functions, such as `glBindBuffer`, are
  in `gl.h` only under `GL_GLEXT_PROTOTYPES`, so each needs an explicit declaration.
- **A function-like macro is not a name.** `SDL_WINDOWPOS_CENTERED` is written as its
  value, `0x2FFF0000`.
- **A C struct read by offsets** is fragile. `SDL_Event`'s fields were checked against
  `offsetof` in C, by hand.
- **No array of n copies of a value.** Five million blocks, and the pixels of each
  texture, start as a loop of pushes.
- **A for-each over a temporary array** is refused as a reference into a temporary:
  `for (string p : s.split(","))` needs the array in a variable first.
- **Casts between `i64` and `f64`** are needed wherever a block coordinate meets a
  position, by the rule that `f64` holds only integers up to 32 bits (DESIGN §9.1).
  `floorI` and `(f64)` appear 124 times.

---

## 7. lisp: an interpreter with a collector

> `lisp.zn`, about 830 lines: a Scheme-like Lisp with closures, calls in tail position
> that don't nest, a REPL, and a mark-and-sweep collector. It was written against the
> compiler, and checked with programs fed on stdin, among them one that churns ten
> million cells. A copy that collects every few allocations prints the same output as
> the real build.

### 7.1 What worked
- **Statuses as the interpreter's errors.** `eval` checks nothing it calls. An unbound
  name, the car of a number, `+#` overflowing or a division by zero fails where it
  happens, and nothing reads the failure, so it leaves every call in progress.
  `i64 value @@failed = lisp.eval(form, NIL);` in the REPL catches it, reports it, and
  the session goes on (DESIGN §7.6). It needed no error type, no checks after calls,
  and no unwinding code.
- **A heap of handles.** Values are `i64`s: tagged integers, or indexes into a
  `Cell[..>]` that only grows. A freed cell goes on a free list and is reused, so
  indexes never move and no reference into the heap outlives anything.
- **The collector in ordinary code.** The mark stack is popped with a repeat loop,
  `while (i64 v : gray ->)`. The roots are an `i64[..~]` that `truncate` cuts back as
  calls return. The ten-million-step loop runs in 4 MB, where it took 4 GB with no
  collector.
- **A switch on strings** picks the built-in, `case "=", "<", ">", "<=", ">=" =>`, and
  a `switch` expression inside an arm gives the comparison.

### 7.2 Fixed in the compiler while writing it
- **A number held in a reference was used as its pointer** in five places: as an
  index, as a slice bound, as a shift count, and as a `switch`'s scrutinee, as a
  statement and as an expression. `src[pos]` with an `i64& pos` stopped the program with
  an index like 140735816223104, out of range. The bounds check kept it from reading
  memory, but the code was wrong. Each place now takes the referenced value's type, as
  string interpolation already did.

### 7.3 What it ran into
- **A `switch` arm can't be `return x;`**, since an arm is an expression or a block:
  `case "car" => { return car(car(args)); }`.
- **Parameters are read-only copies**, so `step`'s tail-call loop works on locals
  copied from them, or here on its slots in the root stack.
- **The roots are by hand.** Every value held in a local across something that may
  allocate has to be on the root stack, and missing one is a bug that shows only when a
  collection happens at that moment. Collecting every few allocations in a test copy
  is what made such a bug show.
- **No enum for a built-in's id.** A built-in cell holds its index in a table of names,
  and dispatch is a switch on the name.
- **A test that edits each line of each example** grows with the square of an
  example's length, so for a long one it now edits about 200 lines spread through it.

---

## 8. hashtree: threads

> `hashtree.zn`, about 150 lines: the SHA-256 of every file under a directory, by a pool of
> workers (DESIGN §12.9), written the day threads were. It matches `sha256sum` on the
> 12,103 files of `/usr/include`, in 0.69 s with one worker and 0.10 s with 18, and
> ThreadSanitizer finds nothing in it.

### 8.1 What worked
- **Workers as lambdas.** `thread.spawn(() => work(jobs, results))` takes the two queues
  along; nothing else had to be declared to be shared.
- **Closing a queue ends the workers.** `while (Job j : jobs.get())` stops at `END` once
  the queue is closed and empty, so no quit message is needed.
- **A limit keeps the walk from running ahead.** `jobs.limit(4 * workers)`, and while a
  `put` fails with `FULL` the main thread collects results, so it never waits on a full
  queue while the workers wait on it.
- **Each worker opens its own Files**, and the incremental `Sha2_256` hashes a file 64 KB
  at a time, with one buffer per worker.

### 8.2 Fixed in the compiler while writing it
- **`append` on a capped array** wasn't generated. It now builds the text at the top of the
  scratch stack and copies it in whole, or fails with `FULL` and appends nothing, which is
  how a path goes into a message: `j.path.append(e.path)`.

### 8.3 What it ran into
- **Text isn't flat.** A path is a view into the walker's buffer, so it travels in a
  `u8[..~4096]`, 4 KB per message whatever its length. Variable-size records through
  queues would carry just the bytes (DESIGN §17).
- **A sort key that is a writable view isn't `ordered`.** `rows.sortBy` needed a helper,
  `nameOf`, taking the names as a `string`, so the key is a read-only view.
