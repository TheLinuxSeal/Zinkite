# What the example programs showed

> Four programs written against `DESIGN.md`, `GRAMMAR.md` v2 and `REFERENCES.md` as of 2026-11-17:
> - `csvsort.zn`: sort CSV rows by a column;
> - `wordfreq.zn`: the most common words on stdin;
> - `linebuf.zn`: an editor line buffer driven by commands;
> - `dirwalk.zn`: total sizes per directory, like `du`.
>
> There is no compiler yet, so they are checked by hand against the grammar and the rules, not compiled.
>
> Section 5 covers a second batch of four, written on 2026-11-19.

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

