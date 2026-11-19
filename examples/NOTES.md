# What the example programs showed

> Four programs written against `DESIGN.md`, `GRAMMAR.md` v2 and `REFERENCES.md` as of 2026-11-17:
> - `csvsort.zn`: sort CSV rows by a column;
> - `wordfreq.zn`: the most common words on stdin;
> - `linebuf.zn`: an editor line buffer driven by commands;
> - `dirwalk.zn`: total sizes per directory, like `du`.
>
> There is no compiler yet, so they are checked by hand against the grammar and the rules, not compiled.

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
2. **Exit status.** Nothing in the design set it, so the examples used `return #1;` from `main`. Proposed: `u8 main(...)` returns the status, `void main` exits with 0, and `exit(code)` ends the program from anywhere (DESIGN §8.6). The examples now return `1`.
3. **Writing to stderr.** Decided: `errprint` and `errprintln`, shorthand for `stderr.print` and `stderr.println` (DESIGN §4.13).
4. **The type of integer literals.** Every loop is written `for (i64 i = 0; ...)`, because lengths and indexes are `i64` while `var i = 0` would have been an `i32`. Decided: with nothing else to decide it, an integer literal is an `i64`, so `var i = 0` is an `i64` (DESIGN §9.1).
5. **Ignoring a bit on purpose.** `f64 parsed @notNumber = f64.parse(key);` declared a variable only to stop the dropped-bit warning, and the variable was then never read. Decided: `_` can be assigned anything and never read, so this is now `num @_ = f64.parse(key);` (DESIGN §7.3).
6. **Arguments a call changes are invisible at the call site.** `File.readAll(path, text)`, `stdin.readLine(line)` and `d.next(names)` all grow their last argument, and only the signature (`u8[..>]&`) says so.
   Decided: no marker, as in Goose. Writing `&text` at the call means the same and draws a warning (DESIGN §5.1).
7. **Maps and the one-growable-field rule.** Decided: a built-in `hashmap<K, V>`, which the compiler gives two data stacks, like a cursor array (DESIGN §4.14). The original problem: a hash map needs a growable table, and rehashing builds a new table while the old one is still live. With one growable field per class (REFERENCES §8.2), the standard library's `map` must either copy every entry twice when it grows or be allowed two growable fields.
   Root structs are no longer needed for maps. They stay open for user classes that need several growable fields (REFERENCES §12).
8. **For-each over library types.** Decided: for-each works over iterators, values with a `mut T next()` method that return a nonzero status at the end, so `codePoints()`, `lines()` and `splitBy()` are lazy (DESIGN §9.4). The original question: `counts` (a map), `s.codePoints()` and `text[:].split('\n')` were all iterated with for-each.
   **Options**: they return arrays, built on a data stack, or they use an iteration interface for classes (Open in DESIGN §17). A lazy `codePoints()` and `split()` would avoid building an array just to loop over it.
9. **Two results from one function.** Decided: tuple results, `(string, string) splitFirst(string s)`, destructured with `(string cmd, string arg) = splitFirst(line[:]);` (DESIGN §9.5). `linebuf` now uses them. The original problem: `linebuf` scans each line twice, once in `firstWord` and once in `afterFirstWord`, because a function returns one value plus a status. A two-field class works, and it would be easy with decision 1.
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
| Input | `stdin.readAll(u8[..>]&)`, `stdin.readLine(u8[..>]&)` |
| Files | `File.readAll(string, u8[..>]&)`, `File.open`, `File.create`, `close` |
| Directories | `Dir.open(string)`, `d.next(u8[..>]&)` returning a `DirEntry { string name; bool isDir; i64 size; }`, `d.close()` |
| Text | `split(u8)`, `lines()`, `splitBy(u8)`, `codePoints()` (the last three are iterators), `compareTo`, `==`, `append(string)`, slices |
| Numbers | `i64.parse`, `f64.parse`, `pad(w)`, `utf8()` |
| Arrays | `push`/`+>`, pop/`->`, `clear`, `sort(lambda)`, `reverse`, `len` |
| Cursor arrays | `+^`, `-^`, `^-`, `move`, `seekStart`, `seekFromEnd`, `cursor` |
| Maps | `hashmap<K, V>` (built in): `getOrAdd(key, default)` returning `V&`, for-each over entries with `key` and `value` |
