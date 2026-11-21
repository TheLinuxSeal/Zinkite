# Zinkite Language Design Notes

> **Name:** Zinkite. The command is `zink` and source files end in `.zn`. Zinkite is a variant spelling of zincite, the zinc oxide mineral: Rust is iron oxide, and zinc is what keeps iron from rusting.
> **Updated:** 2026-11-19. Zinkite now uses a Goose-style memory model: no heap, data stacks freed when their scope ends, values stored inline. The previous design (reference counting with `owned`, `owner` and `weak` references) is kept in full in `DESIGN-refcount.md` and summarized in Appendix A.1. Superseded and rejected ideas are kept in Appendix A with the reasons.

**Status labels**
- **Decided**: settled.
- **Proposed**: recommended during the design discussion, not yet confirmed.
- **Open**: still to be decided (collected in §17).

---

## 1. Goals

- A small, statically typed language for **embedded scripts** and **Linux CLI utilities**.
- **Java-style syntax**: classes with methods in the body, interfaces, constructors, `static` members, lambdas. **No inheritance.**
- **No manual memory management and no memory manager at runtime**: no heap, no allocator, no garbage collector, no reference counts. Memory lives on data stacks that the compiler assigns, and is freed when its scope ends, with one store (§3).
- **Memory safety, checked at compile time**: no use-after-free and no dangling references, without lifetime annotations (§5.2).
- **Runtime safety**: no undefined behavior. Expected failures are reported through each call's status code (§7). Bugs, such as an index out of range, stop the program with a message (§7.6). Nothing is thrown.
- **Fast data layouts**: nested data stored inline and contiguously, references that stay valid while arrays grow.
- Small, fast-starting static binaries.

Left out to stay light: inheritance, exceptions, destructors, reflection, dynamic class loading, and operator overloading beyond a few basics.

**Platform**: 64-bit Linux with virtual memory, since each data stack is a large address-space reservation (§11). Microcontrollers without an MMU are out of scope.

---

## 2. Decision summary

| Area | Decision | Status | § |
|---|---|---|---|
| Memory model | No heap: values live on the native stack, in static data, or on compiler-assigned data stacks; a scope's memory is freed by one store when it ends | Decided | 3 |
| Nested data | Stored inline; a record is a contiguous run of bytes | Decided | 3.2 |
| Array types | Slot grammar `T[sce cap]`: one character each for start, cursor and end, `>` insert, `<` remove, `~` both, `.` nothing; `T[a]` applies one character to all three; a trailing `$` (`T[.~.$]`) chooses a chunked index | Decided | 4.1 |
| Varint lengths | `*` after the slots gives a frozen array a varint length field: `T[.*]` | Decided | 4.6 |
| Array operations | Methods and operators, both: `push(x)` and `a +> x` | Decided | 4.7, 4.8 |
| Cursor arrays | On a data stack, a cursor slot other than `.` gives a data array whose elements never move, plus a gap buffer of indices; references survive inserts and seeks; removal holes are reused | Decided | 4.3, 4.5 |
| Indexing from the end | `a[^1]` is the last element; no negative indices | Decided | 4.10 |
| Assignment | `=` copies a value; references are written `T&`; `.=` rebinds a reference | Decided | 5.1 |
| Binding references | `var& x = e` binds a reference; arguments are never marked, and `&x` at a call draws a warning | Decided | 5.1 |
| Reference safety | Every reference has an inferred root and can't outlive it; removing from an array while a reference into it may still be used is a compile error; follows Goose, with no annotations and no aliasing rules (`REFERENCES.md`) | Decided | 5.2 |
| Links inside data | `T&` fields, rooted like any reference; relative links `T&<i32>` for compact structures, copied only with their whole array; saving them as bytes is Open | Decided | 5.4 |
| Classes | Methods in the body, constructors `new(...)`, `mut` methods, `private`; no header, so a class is exactly as large as its fields | Decided | 6.1 |
| Default constructors | A class with no constructor gets one taking each field without an initializer, in declaration order | Decided | 6.2 |
| Constructors | Every field assigned on every path, checked for the worst case; `this.new(...)` delegates; a failing `return #code;` gives the zero value | Decided | 6.2 |
| Roots | `root Parser { ... }` is a class whose growable fields each get their own data stack; it can't be copied or stored by value | Decided | 6.6 |
| Overloading | Methods, static functions and constructors, by parameter types; chosen at compile time from the arguments' static types; no default parameter values | Decided | 6.5 |
| Dispatch | Interfaces only, no inheritance; an interface reference is 16 bytes (value pointer + method table) | Decided | 6.3 |
| Errors | A `u8` status per call, 0 for success: `return #status.NOT_FOUND x;` writes it, `x @err = f();` reads it; no exceptions | Decided | 7 |
| Status codes | Follow Linux's `errno` numbers (`NOT_FOUND` is `ENOENT`, 2); 1 means failed; Zinkite's own codes from 192 to 223; a program's own from 224 to 255 | Decided | 7.7 |
| Value on failure | Always defined: every return supplies a value, and the standard library returns the zero value on failure | Decided | 7.5 |
| Discarding | `_` takes any value or status and can never be read | Decided | 7.3 |
| Bugs | Overflow, an index out of range, division by zero and similar stop the program with a message | Decided | 7.6 |
| Cleanup | No destructors, as in Goose; `defer` runs a statement when its block ends; the runtime flushes and closes files still open at exit | Decided | 6.4 |
| Number types | `i8`–`i128`, `u8`–`u128`, `f32`, `f64`, plus `bool`; no aliases; lengths and indexes are `i64` | Decided | 9.1 |
| Literals | An integer literal is an `i64` and a float literal an `f64` unless where it is used says otherwise; Rust-style suffixes (`255u8`, `1.5f32`) | Decided | 9.1 |
| Casts | `(u8) n` stops when the value doesn't fit; `(u8%) n` wraps and `(u8\|) n` saturates; `u8.from(n)` fails with `RANGE` instead | Decided | 9.1 |
| Mixed integer types | Allowed when one type holds every value of the other, and the result has that type; otherwise a cast; comparisons between any two integer types are exact | Decided | 9.2 |
| Generic bounds | Built in: `sint`, `uint`, `int`, `float`, `number`, `ordered`, `hashable`, combined with `&`; a class is `ordered` by declaring `compareTo` | Decided | 10.1 |
| Characters | No `char` type: a code point is a `u32`, and `'a'` is an integer literal (ASCII-only when used as a `u8`) | Decided | 9.1 |
| Hash maps | Built-in `hashmap<K, V>`: insertion-ordered, entries never move on insert, the index is rebuilt on growth (Python's compact dict) | Decided | 4.14 |
| Printing | `print` and `println` take text; values are formatted by methods inside `${...}`, such as `${n.hex()}` and `${c.utf8()}` | Decided | 4.13 |
| Strings | `string` is a view (`u8[:]`); text is stored as `u8` arrays | Decided | 4.11 |
| Lambdas and `switch` cases | `(i32 a, i32 b) => a + b`, with every parameter's type written; `case 0 => ...`; `->` is the pop operator | Decided | 9.3, 9.4 |
| Undecided features | The compiler rejects a program that uses a language feature still open in §17 or `GRAMMAR.md` §8, and says the feature is undecided | Decided | 17 |
| Standard library | Packages under `zink.*`; text that is read goes into caller buffers, computed text is built in place, failures are status codes; JSON, JSON5, TOML and YAML share one parsed tree | Proposed | `STDLIB.md` |
| Implementation | Compiler in Rust, runtime in C; the compiler emits C, compiled by the system C compiler, clang or gcc: at `-O0` for `zink run`, at `-O2` for `zink build` | Decided | 12.5, 14 |
| Output | Native code, through C; the embedding model is Open (§13) | Decided | 12, 13 |
| Program settings | One setting per program, as `#` directives at the top of the file containing `main` | Decided | 8.2 |
| Conditional compilation | `#if` / `#else` / `#endif` | Decided | 8.3 |
| Packages and imports | Java-style packages that match directories; `import a.b.C as D` for aliases | Decided | 8.4, 8.5 |
| Script files | A file with no class declaration is allowed; its top-level methods and fields go into a hidden class | Decided | 8.6 |
| Exit status | `u8 main(...)` returns it; `void main` exits with 0; `exit(code)` ends the program from anywhere | Decided | 8.6 |
| Static initialization | Every static field is set before `main`, in dependency order; initializers that depend on each other in a loop are a compile error | Decided | 8.7 |
| String interpolation | `"${expr}"`; otherwise `$` only marks a chunked index in types (§4.1) | Decided | 4.11 |
| Statements and expressions | Java's, plus `var`, for-each, arrow-only `switch`, simple enums | Decided | 9.4 |
| Conditions | Unsigned integers work as conditions: 0 is false, anything else true | Decided | 9.4 |
| Repeat loops | `while (T x : call)` makes the call before every pass and runs while its status is 0: `while (Pending p : todo ->)` | Decided | 9.4 |
| Iterators | A value with `mut T next()` is an iterator, for for-each and repeat loops; `Iterator<T>` for generic code | Decided | 9.4 |
| Multiple results | `(string, string) f()` returns a tuple, and `(string a, string b) = f();` destructures it; the status stays a `u8` | Decided | 9.5 |
| Index modes | `a[i]` checked, `a[%i]` wraps, `a[\|i]` clamps, `a[^k]` from the end | Decided | 4.10 |
| Slices | `a[i:j]`, `a[i:]`, `a[:j]`, `a[:]`; `T[:]` is a read-only view and `T[:]&` a writable one; an array converts to a slice wherever one is expected | Decided | 4.10 |
| Name | Zinkite; command `zink`, source files `.zn` | Decided | — |
| `zink run` | Compiles to a cached native binary with the system C compiler, then `exec`s it | Decided | 14.4 |
| Grammar | `GRAMMAR.md`, version 2 (version 1 kept in `GRAMMAR-refcount.md`) | Decided | `GRAMMAR.md` |

---

## 3. Memory model: data stacks

### 3.1 Data stacks (Decided)
A program has the native call stack, static data, and N **data stacks**, where the compiler works out N. A data stack is a large address-space reservation with a bump pointer, and there is no other memory.

```
for (i32 round = 0; round < 3; round++) {
    u8[..>] scratch = [];                     // grow-only, on a data stack
    for (i32 i = 0; i < 100000; i++) { scratch.push((u8) (i % 256)); }
    println("round ${round}: ${scratch.len} bytes");
}                                               // freed: one store to the stack's top
```

- **At most one resizable value is live per data stack, and it is always on top** (rule G, `REFERENCES.md` §8). Growth therefore never moves anything and never checks a capacity. The compiler proves this; the runtime tracks only the bump pointers.
- **Freeing is one store.** When a scope ends, each data stack it used gets its saved top back, however much data was built and however deeply it is nested.
- **References survive growth.** A reference into a growing array stays valid as long as the array does, because its elements never move:
  ```
  Item[..>] items = [];
  Item& first = items.push(new Item(1, 0.5));   // a reference to element 0
  for (i32 i = 2; i <= 1000000; i++) { items.push(new Item(i, 0.0)); }
  first.heavier(99.0);                          // still valid, a million pushes later
  ```
- Pages are committed by the kernel on first touch, so an unused reservation costs nothing but address space.

### 3.2 Nested data is stored inline (Decided)
A value is stored where it is declared: in a local, a field, or an array element. A class with variable-size parts (text in a `u8[.*]`, a frozen array) stores them inline, so a record is a contiguous run of bytes with no pointers in it:

```
class Item  { u8[.*] sku; i32 qty; i32 cents; }
class Order { i32 id; u8[.*] customer; Item[.*] items; }

Order[..>] book = [];
book.push(new Order(1001, "alice", [new Item("SKU-441", 2, 1999), new Item("SKU-7", 1, 500)]));
```

The whole order book is one block, so reading it streams through the cache.

- **A class with a variable-size field is itself variable-size.** An array of it can be iterated but not indexed, since element `i` has no fixed offset. To index it, keep a second array of references to its elements (§15 measured this: sorting 1,000,000 such records through an array of references).
- Fixed-size alternatives keep indexing: a capped array (`u8[..~16]`) or a view (`string`, §4.11).

### 3.3 Building in place (Decided)
- **A value is constructed at its final destination**, through any depth of calls. `items.push(parse(line))` writes the parsed record straight into the array.
- **Returning a growable array by value doesn't copy it**: the callee builds it where the caller will keep it.
- `new` therefore means "construct here", never "allocate" (§6.1).

### 3.4 What the model costs (Decided, as known trade-offs)
- **Memory goes back to the data stack only when its scope ends.** An array keeps the slots it frees for its own later inserts: popped slots, and a cursor array's holes (§4.5). An array that once held a million elements keeps that memory until its scope ends, even if it now holds ten.
- **Long-running programs need structure.** Per-iteration data goes in an inner scope. A structure that keeps changing for the whole run, such as a cache or a server's state, needs a pool (§3.5) or a periodic rebuild into a fresh array.
- **One resizable value per data stack.** Two growing arrays at once take two stacks, which the compiler assigns. A class with several growable fields is a root (§6.6). An array whose elements are themselves still growing (a list of lists where any inner list can grow later) isn't possible: inner arrays are frozen or capped once their element is pushed. Build each inner array in a scratch array first, then push it (`REFERENCES.md` §8).
- **One growable field per class**, and it must be the last field (`REFERENCES.md` §8.2).
- **Recursive functions can't keep a growable local across a recursive call.** Data that has to outlive the calls is passed in by reference from outside the recursion (`REFERENCES.md` §8.4).
- **Removing from an array while a reference into it may still be used is a compile error** (§5.3).
- **Links inside data follow the reference rules** (§5.4): a reference in a field can't outlive what it points into, so objects can't point freely at each other.
- **Virtual memory is required** (§1).

### 3.5 Pools for long-running programs (Decided)
A built-in `pool<T>` pairs a grow-only array with a free list: `add` reuses a free slot or pushes a new one, and `free` returns a slot (`STDLIB.md` §7.5). A slot's memory is never released while the pool lives, so a stale reference to a freed slot reads a different value of the same type: a logic bug, never memory corruption. This is Goose's `slot_pool`.

---

## 4. Arrays and hash maps

### 4.1 Type grammar (Decided)
```
T[ s c e  cap? ]        three slot characters: start, cursor, end
T[ a  cap? ]            one character, standing for all three: T[~] is T[~~~]
```

Each slot is exactly one character, saying what can change at that position:

| Symbol | Meaning |
|---|---|
| `.` | nothing can change here |
| `>` | insert |
| `<` | remove |
| `~` | insert and remove |

- **The shorthand**: one character stands for all three slots. `T[.]` is a frozen array (`T[...]`), and `T[~]` can do everything (`T[~~~]`).
- **Two characters is an error**, so a type always has one or three slot characters, before any cap or `$`.
- **A `$` after the slots chooses a chunked index** (Decided): `T[.~.$]`, or `u32[~$]` with the shorthand. It goes where a cap would, like `*`, since it can't be combined with one. The operations are the same as without it, and only their costs change (§4.3, §15).
  - **Edits anywhere cost O(√n)**: `insertAt(i, x)` and `removeAt(i)` don't move a gap, and `seek` is free, because the cursor is just a number.
  - **Typing in one place gets slower**, and reads take one more load.
  - **Use it for large arrays edited at scattered positions**: it measured faster from about 50,000 elements up, and 12–16× faster at a million.
  - **It needs a cursor slot other than `.`**, since that's the part it changes, and **it can't have a numeric cap**, since capped arrays are small enough for one gap to win.
  - `$` looks like an S, for "segmented". It can't be confused with interpolation, which happens only inside string literals.

The optional trailing `cap`, or `$` in its place:

| Cap | Meaning |
|---|---|
| a number or compile-time constant (`16`, `MAX_NAME`) | inline storage with that capacity |
| `*` | variable-width (varint) length field, frozen arrays only |
| `$` | chunked index, cursor arrays only (above) |
| *(none)* | no inline cap: the array lives on a data stack (editable) or is sized at construction (frozen) |

The cap says **where the data lives** (inline or on a stack). The slots say **what operations are allowed**.

### 4.2 Type table (Decided)

| Type | Meaning |
|---|---|
| `i32[.8]` | inline, exactly 8 elements, fixed length |
| `T[.]` | frozen: length chosen at construction, fixed-width length field |
| `T[.*]` | frozen, with a varint length field |
| `u8[..~16]` | inline, up to 16 elements, insert and remove at the end |
| `T[..>16]` | inline, append-only, up to 16 |
| `T[..>]` | on a data stack, append-only |
| `T[..~]` | on a data stack, insert and remove at the end |
| `T[>..]` | on a data stack, insert at the start only |
| `T[>.>]` | on a data stack, insert at both ends |
| `T[<.<]` | remove-only from both ends: a window that only narrows, filled elsewhere |
| `T[~.~]` | on a data stack, a deque: insert and remove at both ends |
| `T[>~>]` | insert at both ends, insert and remove at a cursor |
| `T[~]` | everything: a deque at each end plus a cursor |
| `T[.~.$]` | a cursor array with a chunked index: edits anywhere in O(√n) |

There is no `T[k]`: `T[.k]` is the only fixed-size spelling.

### 4.3 Layouts the compiler picks (Decided)

| Pattern | Layout | Notes |
|---|---|---|
| `..>` / `..~` | plain array on a data stack that grows (and shrinks) at the end | one load per read; references stable |
| `>..` | reversed array; the compiler flips the index math | free, no extra memory |
| `>.>` | double-ended: starts in the middle of its stack's reservation, the front pointer moves down, the back pointer up | `a[i]` is `front + i`; must own its stack, so it can't be stored inline in a class |
| a cursor slot other than `.` | a data array whose elements never move, plus an index array with a **gap buffer** at the cursor | reads take two loads; the start and end slots decide whether the index array is double-ended |
| capped (a number or constant after the slots, such as `..~16`) | everything inline in the parent; for a start slot, one block at the top of the buffer when only the start changes, else a ring buffer; a small gap buffer for a cursor, kept apart from the cursor so the gap moves only for an edit there | nests inside classes; counter width chosen from `k` |
| `<.<` | front and back offsets over existing data | a window that only narrows |
| a trailing `$` | the same data array, with the index split into chunks of about √n entries (a two-level tiered vector, Goodrich and Kloss 1999) | inserts and removals anywhere cost O(√n); reads take three loads; the cursor is just a position |

- **Parameters take capabilities, not layouts** (Decided). Since a plain parameter is a read-only copy (§5.1), an array the callee changes is passed by reference. A `T[..>]&` parameter accepts any array that can at least be appended to, and the function can only append. The slots govern only adding and removing elements, so the function may still write and reorder them, as a sort does. The compiler compiles the function once per layout passed to it, since the whole program is visible.
- **Reads on a cursor array cost more even when the code never moves the cursor.** Elements stay where they were inserted, so `a[i]` reads the index array first. Arrays that are mostly read should use a type whose cursor slot is `.`.
- **A lint for unused capabilities** (Decided): a warning when an array's type allows something the code never does. The cursor matters most: `T[~]` is the shortest spelling, but its cursor slot makes every read take two loads (§4.3). The warning would suggest `T[~.~]` when the cursor is never used. It could also suggest adding `$` to a large array whose edits jump around, and removing it when edits stay in one place.

### 4.4 Cost model (Decided)

| Operation | Cost |
|---|---|
| push / pop at the end, pushFront / popFront at the start | O(1) |
| insert / remove at the cursor | O(1) |
| `seek(i)` (move the cursor) | O(distance), moving 4-byte indices |
| insert at a random position (seek, then insert) | O(n) worst case |
| `a[i]` | O(1); two loads if there is a cursor slot |

Nothing gives O(1) insert anywhere together with O(1) indexing: about log n / log log n per operation is the proven lower bound (Fredman and Saks, 1989). Measured (§15): random inserts were still 2–43× faster than a plain array with `memmove`, because a seek moves 4-byte indices instead of whole elements. Random reads were about twice as slow.

### 4.5 Cursor removal and hole reuse (Decided)
- Gap layout only, no tombstones, so indexing stays O(1) and order is preserved.
- Removing widens the gap, and the element's slot in the data array becomes a hole.
- **Holes are reused.** A removal puts the slot on a free list, and the next insert takes a slot from there before appending. The data array therefore never holds more slots than the most elements that were live at once, instead of growing with every insert.
- **The free list lives inside the holes** when an element is at least 4 bytes. Smaller elements keep it in a small side array (Decided).
- **The reuse is never visible**: removing from a cursor array while a reference into it may still be used is a compile error (§5.3, `REFERENCES.md` §5). On a data stack, references survive inserts and seeks, which never move elements. A capped cursor array's small gap buffer does move them (§4.3), so an insert into one ends references into it (§5.3).
- **A cursor slot needs fixed-size elements**, so a reused hole always fits.
- No swap-remove: it would break the order.

### 4.6 Varint lengths (Decided)
A varint (LEB128-style) stores the length in 1 byte up to 127, 2 bytes up to 16,383, and so on. It saves space for many short strings or lists stored inline. The costs:
- the array's size is no longer constant;
- reading the length is slightly slower;
- any class containing one becomes variable-size, so arrays of it can be iterated but not indexed (§3.2).

`*` is allowed on frozen arrays and is illegal with a numeric cap, since the cap determines the counter width. Whether unbounded editable arrays (`T[..>*]`) may carry it is Open (§17).

### 4.7 Methods (Decided)
Which methods exist follows the slots; the compiler rejects any method the type doesn't allow.

| Slot | Insert (`>` or `~`) | Remove (`<` or `~`) |
|---|---|---|
| Start | `pushFront(x)` | `popFront()` |
| End | `push(x)` | `pop()` |
| Cursor | `insert(x)` | `removeBefore()`, `removeAfter()` |

- `push`, `pushFront` and `insert` return a reference to the new element. On a full capped array they fail with `status.FULL` and store nothing, and the reference points at the last element instead. A cap must be at least 1, so a full array always has one, and the reference is safe to use (§7.5).
- `pop`, `popFront`, `removeBefore` and `removeAfter` return the removed value. On failure they return the zero value with `status.EMPTY` (§7.5).
- Every editable array has `len`, `clear()` (one store on a data stack), `a[i]` (except an array of variable-size values, §3.2), and for-each in logical order.
- Frozen arrays have only `len`, indexing and for-each.

### 4.8 Operators (Decided)
The sign says insert (`+`) or remove (`-`); the symbol says where: `<` start, `^` cursor (the caret), `>` end.

| Operator | Method |
|---|---|
| `a +> x` | `push(x)` |
| `a +< x` | `pushFront(x)` |
| `a +^ x` | `insert(x)` |
| `a ->` | `pop()` |
| `a -<` | `popFront()` |
| `a -^` | `removeBefore()` |
| `a ^-` | `removeAfter()` |

- **Insert operators return the array, so they chain**: `a +> 1 +> 2 +> 3` appends in order.
- **Remove operators are postfix** and return the removed value, so they end a chain.
- Each operator needs the same capability as its method.
- **How they fit with the rest of Zinkite** (Decided):
  - `->` is only the pop operator. Lambdas and `switch` cases use `=>` (§9.3, §9.4).
  - `^` marks the cursor, so `+|`, `-|` and `*|` stay saturating arithmetic (§9.2), and `|` keeps meaning "clamp", as in `a[|i]`.
  - `^` also appears inside brackets, where `a[^1]` counts from the end (§4.10). The two uses never appear in the same position.
  - `a^-b` lexes as `a ^- b`, so an XOR with a negated value needs the space: `a ^ -b`. The compiler error suggests it.
  - `+<` can read as "plus less-than"; test it in real code.

### 4.9 Cursor API (Decided)

| Method | Meaning |
|---|---|
| `seek(i)` | move to position `i`, 0 through `len` (a cursor sits between elements, so n elements have n + 1 positions) |
| `seekFromEnd(k)` | move to position `len - k`; `seekFromEnd(0)` is the final gap |
| `seekStart()` | alias for `seek(0)` |
| `cursor` | property: the current position, O(1) |
| `move(n)` | relative move (positive right, negative left), clamped at the edges |
| `insert(x)`, `+^` | insert at the cursor |
| `removeBefore()`, `-^` | remove the element behind the cursor (backspace) |
| `removeAfter()`, `^-` | remove the element ahead of the cursor (delete key) |
| `insertAt(i, x)` | `seek(i)`, then `insert(x)` |
| `removeAt(i)` | `seek(i)`, then `removeAfter()` (removes element `i`) |

- `seek` takes only positions from 0 to `len`; anything else fails with `status.POSITION` and leaves the cursor where it was.
- There is no `seekEnd()`: `seekFromEnd(0)` covers it.
- The `At` forms cost O(distance), or O(√n) with a chunked index (`$`); they aren't random-access inserts.
- The cursor methods require a cursor slot other than `.`.

### 4.10 Indexing (Decided)
- **`a[i]`** is checked. Out of range stops the program (§7.6).
- **`a[^k]`** is element `len - k`, so `a[^1]` is the last element. `a[^0]`, or `k > len`, is out of range.
  - `^k` is bracket syntax only: not a value, can't be stored or passed.
  - Slices can use it: `a[2:^1]`.
- **`a[%i]` wraps**, using true modulo, so `a[%len]` is the first element. **`a[|i]` clamps** to the first or last element. Both work for writes, and both are still out of range on an empty array.
- **`a.getOr(i, fallback)`** returns the fallback when `i` is out of range.
- **No negative indices**: `a[-1]` is out of range, so a computed index that goes negative never silently wraps. Use `a[^1]` or `a[%-1]`.
- **An index can be any integer type** (Decided). The check rejects any value beyond the length, so `buckets[h % (u64) buckets.len]` needs no cast back to `i64` (§9.2).
- The asymmetry is deliberate: `a[^1]` counts elements from 1, while `seekFromEnd(0)` counts gaps from 0.
- **Slices** `T[:]` are views without copying: a pointer and a length, into an array whose elements sit in order in one block. Capped ring and gap buffers are made one block first (below). Slices of cursor, chunked and reversed arrays on data stacks are Open (§17).
- **`a[i:j]` takes a slice** (Decided): the elements from `i` up to, but not including, `j`. `a[i:]` runs to the end, `a[:j]` starts at the beginning, and `a[:]` is the whole array. `^` counts from the end in either bound: `a[1:^1]` leaves out the first and last elements. A bound out of range, or `i > j`, stops the program.
- **An array converts to a slice of its elements wherever a slice is expected** (Decided): as an argument, as a method's receiver, in a declared `T[:]` or `string` variable or field, in a `return`, and as an operand of `==` or another comparison whose other side is a slice. So `[:]` is rarely written:
  ```
  println(out);                          // instead of println(out[:])
  if (line.startsWith("#")) { ... }      // a string method on a u8 array
  for (string l : text.lines()) { ... }
  JsonParser p = new JsonParser(text);
  a.pushAll(b);                          // any element type, not only text
  string first = out;                    // a view of out, exactly as out[:] would be
  var copy = out;                        // var keeps the array's type, so this copies
  if (cmd == "quit") { ... }             // compares the contents
  ```
  - **The view is rooted at the array**, as `out[:]` would be. Rule R applies unchanged, and its errors name the line that took the view.
  - **An array the code may change converts to a writable view** (`T[:]&`) the same way: `upcase(buf)`.
  - **The array's own methods win** over a slice method with the same signature. In overload ranking the conversion counts below an exact match (§6.5), so an overload that takes the array itself is preferred.
  - **Only arrays whose elements sit in order in one block convert.** Cursor arrays, chunked (`$`) arrays and reversed arrays (`>..`) on data stacks don't (§4.3), and the error says so. Their slices are open (§17).
  - **Capped ring and gap buffers convert by becoming one block** (Decided). When a slice is taken and the elements are split, all of them are joined, not just the slice's: a ring is rotated so its first element is at the start, and a gap moves to whichever end moves fewer elements. The order and the cursor don't change. Later slices find one block and move nothing, so every slice taken since the last edit stays right. An insert or removal ends every slice and reference into the buffer, and a slice, sort or comparison ends references to its elements, which joining can move (§5.3); both are compile errors to use afterwards. The cost is at most one pass over `k` elements, after an edit.
- **`T[:]` is read-only; `T[:]&` is a writable view** (Decided). Through a writable view, elements can be written but the length can't change, as `&` elsewhere means "may change it" (§5.1).
  ```
  rows[1:].sort();                        // sort everything but the header row
  buf[0:4].fill(0);
  buf[at:at + 4] = header;                // = copies into the view; the lengths must match
  void upcase(u8[:]& s) { for (u8& b : s) { b = b.toUpper(); } }
  upcase(line[start:end]);
  u8[:]& w = buf[2:8];
  w .= buf[10:16];                        // .= rebinds the view, as for references
  ```
  - **A writable view can only be taken of an array the code may change**: a local, a `&` parameter, or a field inside a `mut` method. Taking one of a string literal, which lives in read-only memory, of a `T[:]`, or of a plain parameter is a compile error.
  - **`=` copies elements into the view**, as `=` through a reference writes its target. Lengths that differ stop the program, like an index out of range. A copy between overlapping parts of one array gives the right result.
  - **`.=` rebinds the view** to another range.
- **`=` on a view follows the rule for values and references** (Decided). A `T[:]` is a value, so `string w = a; w = b;` makes `w` view `b`. A `T[:]&` behaves like a `T&`, so `=` writes through it and `.=` rebinds it.
  - **Rule R applies as to any view**: removing from the array while a writable view of it may still be used is a compile error (`REFERENCES.md` §9).
  - **Every in-place library method** (the sorts, `reverse`, `shuffle`, `swap`, `fill`) works on a writable view too (`STDLIB.md` §7.1).

### 4.11 Strings (Decided)
- **Text is `u8` arrays, as in Goose**:
  - `u8[..>]` is a builder;
  - `u8[.]` or `u8[.*]` is finished text stored inline in whatever holds it;
  - `u8[..~16]` is a small string inside a class;
  - `u8[:]` is a view.
- **`string` is the name of the view type, `u8[:]`.** It is fixed-size (16 bytes), so arrays of strings can be indexed, and it is what parameters and `main`'s arguments use. A string can't outlive the bytes it views (§5.2). A `u8` buffer can be passed wherever a `string` is expected (§4.10).
- **Literals** are views of static data. `==` compares contents.
- **Views are read-only.** Code that changes text in place uses a writable view, `u8[:]&` (§4.10), or a `u8[..>]&`. A literal lives in read-only memory, so a writable view of one is a compile error.
- **Interpolation** (Decided): `"Processed ${count} files in ${path}"`. Where the text goes decides where it is built:
  - **Outputs and `append` get the parts written directly**, with no temporary at all. That covers `print`, `println`, `errprint`, `errprintln`, writes to files and streams, and `append` to a growable `u8` array. `println("${a} and ${b}")` writes `a`, `" and "` and `b` in turn.
  - **Variables get the text built in place**: `u8[.] msg = "${count} files";` builds it in `msg`'s own storage (§3.3), where it lives as long as `msg`.
  - **Everything else gets an exact-size temporary on a scratch data stack**, released at the end of the statement. That's the case for a plain `string` parameter, as in `log("${n} done")`. Every part is evaluated first, then the text is written once at its final size, so it never grows (rule G). A part that is itself a temporary is finished first and sits below it on the stack.
  - **Keeping a view of a temporary is a compile error**: `string s = "${x}";` would outlive its text, so the error suggests `u8[.] s = "${x}";` (rule S, `REFERENCES.md` §3.3).
- **`split`** returns views into its input, so splitting a line allocates nothing.
- **Indexing gives bytes**: `s[i]` is a `u8`, since a string is a `u8` array. Code points come from decoding: `for (u8 b : s)` walks the bytes, and `for (u32 c : s.codePoints())` decodes the UTF-8 into code points (Decided).
- **Comparing a byte with a character just works**: `if (line[i] == ',')`, because `','` takes the type `u8` there (§9.1).

### 4.12 Parsing notes
- **In a type, the characters between `[` and `]` are read directly**: one or three slot characters, then an optional number, constant or `*`. So `>>` and `<<`, which are tokens elsewhere, count as two slot characters there.
- **A `$` directly before `]` in a type** marks the chunked index (§4.1). Outside string literals, `$` has no other meaning.
- **Types and slices can't be confused.** A slice always has a `:` between its bounds, and the only type with a `:` is the slice type `T[:]`. That one is spelled like the whole slice `a[:]`, and the declaration rule (`GRAMMAR.md` §6, rule 4) tells them apart, as for other types.
- **Slice bounds are shift-level expressions** (`GRAMMAR.md` §6), below comparisons, bitwise operators and the conditional. So a conditional keeps its own `:`, and `a[c ? i : j]` is an index. A bound that uses one of those operators needs parentheses: `a[(c ? i : j):]`, `a[0:(n & 7)]`.
- **One near-collision**: `u8[~16]` (a capped array that can do everything) and `a[~16]` (indexing with the bitwise NOT of 16) are spelled alike. A statement that starts with a type followed by a name is a declaration (`GRAMMAR.md` §6, rule 4), so the parser can tell them apart, but a reader may not.
- Generics written with angle brackets around slot types, such as `hashmap<string, u8[..>16]>`, need an early parser test (§18, test 121).

### 4.13 Printing and formatting (Decided)
```
println("count: ${n}");               // 97        a number prints as a number
println("addr:  ${addr.hex()}");      // 7ffd3a10
println("char:  ${c.utf8()}");        // a         a code point as text
println("line:  ${line.utf8()}");     // hi        a u32 array as text
println("id:    ${id.zeroPad(8)}");   // 00000097
println("cost:  ${x.fixed(2)}");      // 3.14
```

- **`print(text)` writes to stdout, and `println(text)` adds a newline**, as in Java. Both take text, usually an interpolated string. They set no status (Decided), so a bare `println` draws no warning. A failed write is remembered, reported on stderr at exit, and makes the exit status 1 (§6.4). Writing to a closed pipe ends the program quietly (§7.6). Code that must check each write calls `stdout.write(...)` or `stdout.flush()`, which do set a status.
- **`errprint(text)` and `errprintln(text)` write to stderr**, for error messages. They stay on the terminal when stdout is redirected to a file or a pipe.
- **`stdin`, `stdout` and `stderr` are `File` handles** for descriptors 0, 1 and 2, with every method of a file (`STDLIB.md` §4.1), so `File input = stdin;` works. The four functions are shorthand: `println(x)` is `stdout.println(x)`, and `errprintln(x)` is `stderr.println(x)`, except that the four set no status (below).
- **Defaults in `${...}`**: the type decides how a value prints.

  | Value | `${x}` prints |
  |---|---|
  | `string`, any `u8` array | the text |
  | an integer or float | the number, in decimal |
  | `bool` | `true` or `false` |
  | any other array | a list: `[104, 105]` |
  | a class value | a compile error until default formatting is decided (§17); format its fields instead |

- **Formatting methods** change that, and work in any interpolation: in `print`, in a string, in a buffer, or in a file write. Names:
  - `hex()`, `bin()`: the integer in base 16 or 2;
  - `pad(w)`, `zeroPad(w)`: at least `w` characters wide, padded with spaces or zeros;
  - `fixed(d)`: a float with `d` decimal places;
  - `utf8()`: a `u32` code point, or an array of them, encoded as UTF-8 text. A value that isn't a code point (a surrogate, or above 0x10FFFF) is written as `�` (U+FFFD), with `status.INVALID`.
- **They are methods, not global functions**, so common names such as `hex` and `pad` never clash with a program's own. This matches `i32.parse(s)` and `a.divOr(b, 0)`.
- **Methods whose width is fixed by the type return small capped arrays**: `hex()` of a `u64` fits in `u8[..~16]`, and of a `u128` in `u8[..~32]`, so they never touch a data stack. Each is as wide as the type's digits in its base: `bin()` of a `u64` is a `u8[..~64]`.
- **`pad(w)`, `zeroPad(w)` and `fixed(d)` build their text in place** (§3.3), as `utf8()` of an array does, since their length depends on a value known only at run time (Decided). Inside `${...}`, the usual case, the text goes straight to the output with no temporary (§4.11). `u8[.] s = n.pad(8);` builds it where `s` lives.
- **A program's own formatting works the same way**: a method or function that returns text, such as `money(cents)`, goes in `${...}` like `hex()`.
- **No format strings and no format specs** (`printf("%x")`, `${n:x}`), and no separate print function per format (Appendix A.11).

### 4.14 Hash maps: `hashmap` (Decided)
```
hashmap<string, i64> counts = [];               // empty, like an array literal
counts.put("apple", 3);
i64& c = counts.getOrAdd("pear", 0);             // a reference to the value
c++;
i64 n @missing = counts.get("plum");             // 0, with status.MISSING
@missing = counts.remove("apple");               // status.MISSING if it wasn't there

for (var& e : counts) {                          // insertion order
    println("${e.key}: ${e.value}");
}
```

- **`hashmap<K, V>` is built in, like arrays.** It is created by `[]` and needs no `new`. A map is one growable region, so a class can hold one as its one growable field (§3.4), and a root can hold several (§6.6).
- **`hashset<T>` is built in the same way**, with the same layout minus the values (`STDLIB.md` §7.4).
- **Methods**:
  - `get(key)` returns the value, or the zero value with `status.MISSING` when the key is missing;
  - `getOrAdd(key, initial)` returns a `V&`, adding `initial` first if needed;
  - `put(key, value)` adds or replaces;
  - `remove(key)` fails with `status.MISSING` when the key is missing;
  - `has(key)`, `len` and `clear()`;
  - for-each visits entries in insertion order, each with a read-only `key` and a writable `value`.
- **Layout** (Decided): Python's compact dict (since 3.6), kept in one block. It was chosen over Goose's direct table after measuring both (§15): it builds, grows and iterates faster and uses less memory, while the direct table finds keys up to 1.75× faster in maps too big for the CPU cache.
  - Entries are appended in insertion order. Each holds the key, the value, and the key's hash when the key is costly to hash, such as a string.
  - An index of 1-, 2- or 4-byte slot numbers, sized to the table, finds them by linear probing. It sits in the same block, after the entries' spare room.
  - **Growing rebuilds only the index**, after the new spare room, from the stored hashes or by hashing cheap keys again. Entries never move. So a reference from `getOrAdd` or a for-each by reference survives later inserts.
  - **Removing marks the entry as removed**, which keeps the order. When more than half the entries are removed ones, the next removal compacts the array.
- **Reference rules** (`REFERENCES.md` §9):
  - inserts never invalidate references;
  - `remove` and `clear` are removals under rule R, so a reference into the map can't be used after one;
  - like a removable array's, references into a `hashmap` can be held only in local variables and parameters.
- **A random hash seed per run** protects against inputs crafted to collide. Iteration follows insertion order, not hash order, so output is still the same on every run.
- **Keys and values must be fixed-size.** Keys must also be `hashable` (§10.1):
  - integers, `bool` and `string` (a view; its bytes are hashed) are hashable;
  - so are classes whose fields are all hashable, compared and hashed field by field;
  - floats aren't allowed as keys, since `NaN` isn't equal to itself.

  A `string` key is a view, so the text it views must outlive the map (rule S). A program usually keeps that text in one `u8[..>]` buffer, as `wordfreq` does.
- **Values can't grow.** A `hashmap` of growable arrays or of other `hashmap`s is an error, for the same reason as an array of growing arrays (§3.4). The workarounds are:
  - a composite key, as in `hashmap<Pair, i64>`;
  - values that are indexes into one shared growable array.

---

## 5. References

### 5.1 Copies and references (Decided)
```
Item[..>] items = [];
items +> new Item(1, 0.0) +> new Item(2, 0.0);

Item copy = items[0];          // copies the value
copy.heavier(2.5);             // items[0] is unchanged

Item& r = items[0];            // a reference: nothing is copied
r.heavier(2.5);                // changes items[0]
r = new Item(2, 0.0);          // writes a new value into items[0]
r .= items[1];                 // rebinds r to another element

items[0].heavier(1.0);         // works in place, with no copy
```

- **`=` copies.** A reference is declared with `T&`; it can't be hidden, because the memory model depends on knowing what refers to what.
- **References are transparent**: no `*` and no `->`; fields and methods are reached with `.`.
- **`.=` rebinds a reference** (Goose's operator; Decided). `=` through a reference writes the target. Rebinding is how code walks linked data without recursion:
  ```
  Node&? cur = list.head;
  while (cur != null) {
      total += cur.value;
      cur .= cur.next;           // cur = cur.next would overwrite the node instead
  }
  ```
  - `.=` works on reference variables and fields, on link fields (§5.4), and on writable views (§4.10). On a value, it is a compile error that suggests `=`.
- **`var x = e` copies; `var& x = e` binds a reference** (Decided): `var& c = counts.getOrAdd(word, 0);`.
- **`T&?` is a nullable reference** and must be narrowed (`if (r != null)`) before use (Decided).
  - **Narrowing follows the program's flow**: it holds inside `if (r != null)`, and after `if (r == null) { return; }`, or a `break` or `continue` taken when `r` is null.
  - **It applies to local variables and parameters, not to fields**, since a call could change a field. Copy a field into a local first: `Node&? after = n.next; if (after != null) { ... }`.
  - **Rebinding the variable with `.=` ends it.**
- **Parameters** (Decided): a plain parameter is a read-only copy. The compiler passes large ones by pointer when the difference can't be observed. A `T&` parameter lets the callee change the caller's value.
- **Nothing marks a changeable argument at the call site** (Decided, as in Goose). `File.readAll(path, text)` binds `text` to a `u8[..>]&` parameter, and only the declaration says it may change. Writing `&text` at the call means the same, and draws a warning.
- **Interface references** are always references: `Shape&` (§6.3).

### 5.2 Roots: lifetime checks without annotations (Decided)
Every reference and slice has a static **root**: the variable that bounds its target's lifetime. This is rule S (`REFERENCES.md` §3). The compiler infers roots, and a summary for each function of how its result and stores relate to its parameters' roots, so no annotations are needed. There are no aliasing or exclusivity rules. This is Goose's approach, except that functions are compiled once instead of once per root. The full rules are in `REFERENCES.md`.

```
string longest(string a, string b) { return a.len >= b.len ? a : b; }

u8[..>] outer = [];
string w = outer;
{
    u8[..>] inner = [];
    inner.append("inner text");
    w = longest(outer, inner);         // compile error: the result may be rooted at inner,
}                                        //   which doesn't outlive w
```

- A reference must not outlive its root, and must not be stored anywhere that could outlive it. The error names the reference's root and its destination.
- A function can return a reference rooted at a parameter or at a static; at the call site its root is the argument's root. Returning a reference to the function's own local is a compile error.
- References into a growing array stay valid while the array lives (§3.1).

### 5.3 Removal and stale references (Decided)
- **Removing elements from an array is a compile error while a reference into it may still be used.** That covers `pop`, the cursor removals, `clear`, and assigning the whole array. The check follows the program's order, and the error names the reference and the line where it was bound. This is Goose's rule, and rule R in `REFERENCES.md` §5.
- **So freed slots are reused right away**: popped slots by the next push, and cursor-array holes by the next insert (§4.5).
- **References into an array that can remove elements, or whose elements can move, stay in local variables and parameters.** They can't be stored in fields, array elements or statics, so the compiler sees every one. References into grow-only and frozen arrays can be stored anywhere the root rule allows.
- **A reference follows its element** (Decided). An insert that moves nothing ends nothing, at either end: after `i32& x = a[0]; a +< 0;`, `x` is `a[1]` and still reads the same value.
- **Capped arrays follow the same rule** (Decided). A removal ends references into one, as on a data stack. Those whose elements can move, gap buffers (a cursor slot) and rings that change at both ends, which can wrap, also end references and slices at every insert, and a slice, sort or comparison of one ends references to its elements, as it may first make the buffer one block (§4.10). A capped array that changes only at the start sits at the top of its buffer, so like `[..>k]` it never moves an element (`REFERENCES.md` §5.4).
- **Pools are the exception.** A pool's storage never holds another type, so a reference survives a `free` and reads whatever value of the same type is in the slot: a logic bug, never memory corruption.
- **Variable-size values are written once**: a variable-size field can't be assigned. A whole variable can be, which rebuilds the value and counts as a removal (`REFERENCES.md` §6).

### 5.4 Links inside data (Decided)
The notes said "no pointers inside records", but also that `push` returns a reference "so elements can be linked while building". Three kinds of link are allowed, for different jobs (`REFERENCES.md` §7):
1. **References in fields**: `T&`, `T&?`, interface references and slices, rooted like any other reference. An array can hold references rooted at itself, so they also link elements of one array. Each takes 8 bytes, and a copy still points at the original.
2. **Self-relative links**: `T&<i32>` (or `<i16>`, `<i8>`) stores a signed byte offset from the field to its target in the same array, as Goose's relative references do. They are compact, and mean the same thing wherever the whole array is copied. Saving them as bytes needs a check that is Open (`REFERENCES.md` §12). A value with these links can't be copied out of its array, only referenced in place.
3. **Pool links**: `T&<u32 in arr>` stores an offset from the start of `arr`, a static grow-only array, so values holding them can be copied anywhere (Goose's pool-relative references). The built-in `pool<T>` (§3.5) isn't such an array.

Links and references in fields can point only into arrays whose elements stay in their slots: grow-only and frozen arrays, capped ones included, except a capped ring that grows at both ends (`T[>.>k]`), which can wrap. References in fields may also point into pools (`REFERENCES.md` §7.4). A field may also refer to a whole array, as the library's writers do (`REFERENCES.md` §7.1).

Plain indices (`i64`, or `u32` to save space) remain available as ordinary numbers.

---

## 6. Classes and interfaces

### 6.1 Classes (Decided)
```
class Item {
    i32 id;
    private f32 weight;

    new(i32 id, f32 weight) { this.id = id; this.weight = weight; }

    mut void heavier(f32 by) { weight += by; }
    bool isHeavy() { return weight > 10.0; }
    static Item zero() { return new Item(0, 0.0); }
}

Item[..>] items = [];
Item& first = items.push(new Item(1, 0.0));
first.heavier(2.5);
```

- **No header and no hidden fields**: a class is exactly as large as its fields (§11).
- **Methods live inside the class body.** Fields, constructors, methods and static functions appear in any order. There's no adding methods from outside; a free function can be called as `x.helper()`.
- **Constructors are named `new`** inside the body. Call sites write `new Item(1, 0.0)`.
- **`new` means "construct in place at the destination"**, never heap allocation (§3.3). It is only for classes: arrays, primitives and other values come from literals and declarations. Where the target type is known, the class name can be dropped: `items.push(new(1, 0.0))` (Decided).
- **Implicit `this`**: fields are reachable by their bare names.
- **`mut`** marks methods that change the object. Other methods are read-only, which the compiler checks.
- **Visibility**: `public`, `private` (only this class's methods), and package-private when nothing is written. There is no `protected`, since there are no subclasses.
- **`static`** marks functions and fields with no instance.
- **`==` compares fields** (Decided). Values have no identity.
- **Slot capabilities double as contracts**: a method with an append-only array parameter (`T[..>]&`) can't remove from it (§4.3).
- **No inheritance**: `extends` is a compile error. Interfaces (§6.3) provide dynamic dispatch.

### 6.2 Constructors
- **Every field must be assigned before a constructor ends**, and can't be read before it is assigned (Decided). Fields with an initializer, editable arrays, and `hashmap`, `hashset` and `pool` fields, which all start empty, count as assigned from the start. There are no zero defaults for the rest.
  - **The compiler checks the worst case.** A field counts as assigned at a point only when every path to that point assigns it, as in Java's definite assignment:
    - an `if` may go either way, so a field assigned in one branch only, or in an `if` without an `else`, isn't assigned after it;
    - a `while`, `for`, for-each or repeat loop may run zero times, so what its body assigns doesn't count after it. The exceptions are `while (true)` and `for (;;)`, which end only through a `break`: after one, a field is assigned when it is assigned before every `break` that leaves the loop;
    - a `do` loop runs its body at least once, so the body counts;
    - a `switch` counts when it covers every case and every arm assigns the field;
    - `&&`, `||` and `?:` count only what every path through them assigns;
    - only the active branch of an `#if` is checked.
  - **A successful exit counts as an end**: a field must be assigned at each `return;` too.
  - **A failing `return #code;` needn't assign anything** (Decided). The object is then its class's zero value, as the result of any failed call is (§7.5), and fields assigned before it are reset.
    ```
    new(string spec) {
        i64 w @bad = i64.parse(spec);
        if (bad) { return #status.INVALID; }     // fine: the Rect is all zeros, with INVALID
        ...
    }
    ```
    The exception is a field with no zero value: a non-nullable reference (`T&`, `Shape&`), a non-nullable function (`func(i64 => i64)`), which would leave nothing to call, or a value that contains one (Decided). It must be assigned on every path, failing ones included. A nullable one (`T&?`, `func(...)?`) has `null` as its zero value.
  - **A reference field is bound by its first assignment**: in a constructor, `this.next = n;` on a field that isn't assigned yet binds it, as an initializer does. Once it is assigned, `=` writes through it and `.=` rebinds it (§5.1).
  - **`this` can't be used until every field is assigned**, except to assign fields. Calling a method on it, passing it or storing it before then is a compile error, since the method could read a field that isn't set yet.
  - **Locals follow the same rule**: a local declared without an initializer can't be read until every path has assigned it.
- **Default constructor** (Decided): a class that declares no constructor gets one. It takes, in declaration order, every field that has no initializer and isn't an editable array, a `hashmap`, a `hashset` or a `pool`, and assigns each one.
  ```
  class Row { string line; string key; f64 num; }
  class Options { bool numeric = false; i64 column = 0; string path = ""; }
  class Words { string[..>] list; }

  new Row(line, key, num)     // the three fields in order
  new Options()               // every field has an initializer
  new Words()                 // an editable array starts empty, so it isn't a parameter
  ```
  - The order is the order in the source, not the layout order in memory (§11).
  - A class that declares any constructor gets no default one, as in Java.
- A constructor has no written return type. `return;` and `return #1;` are allowed as early exits (§7.2).
- **Constructors can be overloaded** like methods (§6.5): `new(f64 w, f64 h)` beside `new(f64 side)`.
- **One constructor can call another** with `this.new(...)` (Decided):
  ```
  class Rect {
      f64 w; f64 h;
      new(f64 w, f64 h) { this.w = w; this.h = h; }
      new(f64 side) { this.new(side, side); }
  }
  ```
  - It runs at most once on any path. Before it, statements may compute its arguments, but nothing may assign a field or use `this`. After it, every field counts as assigned.
  - It is a call, so its status can be read: `@err = this.new(w, h);`. Dropping one that can be nonzero warns, as for any call.

### 6.3 Interfaces (Decided)
```
interface Shape {
    f64 area();
}

class Circle implements Shape {
    f64 r;
    new(f64 r) { this.r = r; }
    f64 area() { return 3.14159 * r * r; }
}

class Rect implements Shape {
    f64 w;
    f64 h;
    new(f64 w, f64 h) { this.w = w; this.h = h; }
    f64 area() { return w * h; }
}

Circle[..>] circles = [];
Rect[..>] rects = [];
Shape&[..>] shapes = [];                    // references into the two arrays
shapes.push(circles.push(new Circle(1.0)));
shapes.push(rects.push(new Rect(2.0, 3.0)));

f64 total = 0.0;
for (Shape& s : shapes) { total += s.area(); }
```

- **An interface reference is 16 bytes**: a pointer to the value plus a pointer to the method table for that class and interface. Values carry no class id, so plain data stays header-free. This is how Go and Rust represent interface values.
- **An interface is never a value type.** `Shape s;`, a `Shape` field and a `Shape[..>]` are compile errors, because the size isn't known. Store each concrete class in its own array and keep `Shape&` references. Those references follow the root rules (§5.2).
- **Generic bounds are static**: `<T implements Shape>` compiles once per type, with direct calls. The built-in bounds, such as `ordered` and `number`, are in §10.1.
- **`instanceof`** on an interface reference compares the method-table pointer: `if (s instanceof Circle c)` binds `c` as a `Circle&` only when it matches. There are no casts between reference types.
- Open (§17):
  - default methods;
  - static methods in interfaces;
  - storing mixed variants inline. Goose's enums can be fixed-size or each variant's own size.

### 6.4 Resources and cleanup: no destructors (Decided)
```
File out @failed = File.create(path);
if (failed) { errprintln("can't create ${path}"); return #1; }
@failed = out.write(data);
if (!failed) { @failed = out.close(); }   // nonzero (IO, NO_SPACE) if buffered data couldn't be written

File cfg @missing = File.open(configPath);
if (missing) { return #1; }
defer @_ = cfg.close();         // runs when this block ends, however it ends
```

- **No destructors**, as in Goose. Nothing runs when a value's scope ends, so freeing stays one store with no per-element work, and every class can be copied. `delete` is reserved, and declaring `delete()` is an error that suggests `defer`.
- **`defer stmt;`** runs the statement when its enclosing block ends: at the closing brace, or on a `return`, `break` or `continue` that leaves the block.
  - Several defers run in reverse order.
  - A deferred statement can't itself `return`, `break`, `continue` or write a status.
  - It doesn't run when the program stops (§7.6); this is Goose's `defer`.
- **Failures are reported with the status** (§7): `File.create`, `write` and `close` fail with a code such as `status.NOT_FOUND`, `status.PERMISSION` or `status.IO`. A deferred call that can fail needs its status captured or discarded: `defer @_ = d.close();` (§7.3). For a file that was written, call `close()` and check its status.
- **A `File` is a handle** (Decided): a slot and a generation number in a table the runtime keeps, with the buffers in the runtime's own memory rather than in the value.
  - Copying a `File` copies the handle.
  - Closing it through one copy makes every copy's operations fail with `status.BAD_HANDLE`, instead of touching a descriptor that has since been reused.
- **Backstop at exit** (Decided): at normal exit, and when the program stops, the runtime flushes and closes every file still open, and reports any failure on stderr. That includes an earlier failed write by `print`, `println`, `errprint` or `errprintln` (§4.13), which also makes the exit status 1 when it would otherwise be 0. A forgotten `close()` keeps a descriptor open until exit, but never loses data silently.
- **A lint** (Decided) warns when a local `File` may still be open at the end of the scope that opened it, with no `defer` closing it and no store that keeps it.
- There's no `finalize()` and no `with` statement.

### 6.5 Overloading (Decided)
Methods, static functions and constructors can share a name when their parameter types differ. The compiler picks one at compile time from the arguments' static types, as Java does.

```
interface DataWriter {                  // STDLIB.md §9.7
    mut void write(string s);
    mut void write(i64 n);
    mut void write(u64 n);
    mut void write(f64 x);
    ...
}

w.write("id");                      // write(string)
w.write(42);                        // write(i64): an integer literal's own type is i64
w.write(count);                     // a u32 count: write(u64), which keeps it unsigned
w.write(ratio);                     // an f32 ratio: write(f64)

i64 n @bad = i64.parse(s);          // parse(string)
i64 m @bad2 = i64.parse(s, 16);     // parse(string, i64): base 16
```

- **How a call is chosen** (Decided):
  1. The candidates are the declarations with that name and as many parameters as the call has arguments.
  2. A candidate fits when each argument has its parameter's type or converts to it implicitly (§9.1).
  3. Each argument's fit is ranked:
     - best is an **exact** match: the same type, or a literal of its default type (`i64` for integers, `f64` for floats, `u32` for character literals);
     - next is **widening within a kind**: a signed type to a wider signed one, an unsigned type to a wider unsigned one, `f32` to `f64`, or a literal to another type it fits;
     - last come **other conversions**: an unsigned type to a wider signed one, an integer to a float, a class value to a reference to an interface it implements, and an array to a slice of its elements (§4.10).
  4. The chosen candidate fits every argument at least as well as every other candidate does, and some argument better. If no candidate does, the call is ambiguous: a compile error that lists the candidates and suggests a cast.
  5. A non-generic candidate beats a generic one that fits equally well.
- **Static types decide**: a `Shape&` argument calls the `Shape&` overload even when it refers to a `Circle`. Dynamic dispatch comes only from interface methods (§6.3).
- **Overloads must differ in a parameter's type.** Differing only in `&`, `?`, an array parameter's slots, the return type or the parameter names is a compile error, since one call could fit both: `f(Item x)` beside `f(Item& x)` is an error. A slice and an array are different kinds, so `f(string)` beside `f(u8[..>]&)` is allowed, and a buffer argument calls the second, its exact match.
- **Generic bodies choose once, against their bounds** (§10). An argument of type `T` fits a parameter of type `T`, or a generic parameter whose bound `T` satisfies, but never one overload per number type. So `<T implements number> void emit(JsonWriter& w, T x) { w.write(x); }` is an error: no single `write` takes every `number`.
- **Lambdas carry their types** (§9.3), so a lambda fits a `func` parameter only when the types are the same, and overloads on function types are chosen like any others.
- **Overloads should do the same thing for different inputs.** Different behaviour gets a different name, so `sort(less)` and `sortBy(key)` stay apart (`STDLIB.md` §7.1).
- **Interfaces can declare overloads**, and a class implements each one.
- **Not included**: default parameter values, and overloaded operators (§1).

### 6.6 Roots: several growable fields (Decided)
A class keeps its fields in one block, so it can hold at most one growable field, at its end (§3.4). A **root** is declared like a class, with `root` in place of `class`, and gives each growable field its own data stack instead. It follows Goose's root structs.

```
root Parser {
    Token[..>] tokens;              // its own data stack
    i64[..~] work;                  // another one
    u32[.~.] text;                  // a cursor array: its data and its index get a stack each
    i64 position = 0;               // ordinary fields stay in the root's record

    mut void reset() { tokens.clear(); work.clear(); position = 0; }
}

Parser p = new Parser();            // only as a named local or static
parse(p, input);                    // void parse(Parser& state, string input)
```

- **A root can have everything a class can**: fields, methods, constructors, `implements` and type parameters. Its growable fields may come in any number and any order.
- **Its value is a small record of where each part lives**, not one block of bytes, so it can't be copied. These are compile errors:
  - copying it (`Parser q = p;`), passing it as a plain parameter, or returning it by value;
  - storing it by value in a class, another root or an array;
  - an unnamed temporary: `new Parser()` must initialize a named local or static.

  References to it (`Parser&`) go wherever references can, under the usual rules.
- **The number of data stacks stays fixed at compile time.** Each root variable's parts get stacks the compiler assigns, as growable locals do, and roots whose lifetimes don't overlap reuse them. That is why a root can't be an array element.
- **Recursion**: a root can't be live across a call into a recursive cycle that would need new stacks on each activation. Pass one declared outside the cycle by reference (`REFERENCES.md` §8.4).
- **In a class, a second growable field is a compile error that suggests `root`, and so is a cursor-array field**, since a cursor array needs two regions: its data and its index.
- **Speed**: each part is the top of its own data stack, as a local array is, and the compiler keeps each part's start and length in registers inside loops. Goose measured its root structs against passing the same arrays separately, over 32 million pushes: 85–88 ms either way.
- **`root` is a keyword only at the start of a type declaration**, so variables and fields named `root` stay legal: `Node& root = tree.push(...)`.
- **References into a part are rooted at that part** (Decided): a reference into `p.tokens` has the root `p.tokens`, which lives as long as `p`. A root's parts are separate storage, so rule R checks a removal from one part only against references into that part, and the pool exemption (`REFERENCES.md` §5.4) applies to references into a pool part (`REFERENCES.md` §8.2). A field of any class holding an array that can remove or move elements is separate storage in the same way (`REFERENCES.md` §5.3).
- **The name**: a root owns its parts' storage, so its variable bounds the life of every reference into them (§5.2).

---

## 7. The status code (Decided)

Every function has an optional `u8` side output, its **status**, separate from its return value. 0 means the call succeeded. Any other value means it failed, and says why.

### 7.1 Semantics
- **The default is 0.** A function that never sets a status returns 0.
- **By convention, nonzero means something went wrong**: 1 for a failure with no particular reason, and the standard library's named codes (§7.7) when there is one. Programs may use the status for anything else.
- **The status belongs to the call, not the function.** It is used at the call site or dropped.
- **Nothing propagates automatically**; passing a status on is explicit.
- **Implementation**: a hidden second return value in a register. Functions that never set a status carry none.
- **The standard library fails with a named code** (§7.7):
  - for failed I/O, the operating system's own error number, such as `NOT_FOUND` (`ENOENT`) or `PERMISSION` (`EACCES`);
  - `EMPTY` for a pop on an empty array, and `FULL` for a push on a full capped array;
  - `MISSING` for a key or element that isn't there;
  - `POSITION` for an out-of-range cursor move;
  - `END` at the end of input or of an iterator.

### 7.2 Writing a status: `#`
```
return #1 value;                   // failed, for no particular reason
return #status.NOT_FOUND value;    // failed, saying why
return #0 value;                   // explicit success (same as plain `return value;`)
return #(err) value;               // a u8 expression, such as another call's status
return #(missing) value;           // a bool counts as 0 or 1
return #1;                         // early exit from a void function or constructor
```

- **`#` takes** one of these:
  - an integer literal from 0 to 255;
  - a named code (`#status.FULL`), or a `u8` or `bool` variable (`#why`);
  - `#( ... )` holding a `u8` or `bool` expression.

  Other integer types are an error: write `#((u8) n)` or `#(n != 0)`.
- **A bare `@(...)` after `return`** is a compile error ("use `#(...)` to pass the status on").
- **Falling off the end of a function** returns status 0.
- **There's no separate `flag` statement** for "mark now, return later". It can be added later without breaking anything (Appendix A.8).

### 7.3 Reading a status: `@`
```
x @err = a.pop();             // value into x, status into err
@err = a.pop();               // status only, value dropped
x = a.pop();                  // value only, status dropped
@err = a +> 1 +> 2;           // the first failure in a whole chain
u8 bad = @a.pop();            // @ in an expression gives the status directly
if (@a.pop()) { ... }         // nonzero means the call failed (§9.4)
if (@(a +> x)) { ... }        // parentheses around operator expressions
if (err == status.EMPTY) { ... }
```

- **`@expr`** means "evaluate `expr` and give me its status". The call still runs; its value is discarded.
- **`@` is a high-precedence prefix operator** on the whole postfix expression, so `@a.pop()` means `@(a.pop())`. Binary-operator expressions need parentheses.
- **On the left of `=`, `@name` names a destination**; in an expression, `@` reads a status. The parser tells them apart.
- **`err` is always a `u8` and needs no type.**
  - In a declaration (`Item x @err = a.pop();`), it is declared along with `x`.
  - In an assignment (`x @err = ...`, `@err = ...`), it must already exist.
  - `u8 err = @a.pop();` declares a status on its own (`GRAMMAR.md` §5, §6).
- **Unsigned integers work as conditions** (§9.4), so `if (err)` tests whether the call failed.
- **`while (T x : call)` repeats a call until its status is nonzero** (§9.4).
- **A bare call in a condition tests its value**; `@` in front tests its status.
- **Dropping a status that can be nonzero** gives a warning. Capturing it with `@err = ...` acknowledges it.
- **`_` discards** (Decided). It can be assigned anything, and it can never be read:
  ```
  num @_ = f64.parse(key);        // keep the value, ignore the status on purpose: no warning
  @_ = out.close();               // ignore the status of a call made for its effect
  _ = lookup(key);                // ignore a value
  _ @_ = f();                     // ignore both
  defer @_ = d.close();           // a deferred call, without the dropped-status warning
  func(i64, i64 => i64) first = (i64 a, i64 _) => a;   // a parameter that isn't used
  ```
  - It declares nothing and has no type, so it can be assigned a value of any type, any number of times.
  - It works as an assignment target, a status destination, a parameter name, a destructuring target, a for-each variable (`for (var _ : items) { n++; }`) and a repeat-loop variable.
  - Reading it, as in `if (_)` or `x = _`, is a compile error. So are compound assignments such as `_ += 1`, which would read it.
  - Only a lone `_` is special: `_count` and `my_value` are ordinary names.

### 7.4 Chaining and combining
- **Chains keep the first failure**: the status of `a +> 1 +> 2` is the first nonzero status in it, and later steps never overwrite it.
- `@a.pop() || @b.pop()` is true when either failed; `&&` when both did. These are easy to get backwards, so the documentation says so.
- **In `@f(g())`, the status is `f`'s.** `g`'s status is dropped, with a warning if it can be nonzero.

### 7.5 The value on failure (Decided)
- **The value is always defined.** Every non-void `return` supplies a value, whatever its status, so `x @err = f();` never leaves `x` undefined.
- **The standard library returns the zero value on failure**: `0`, `false`, an empty view, or the zero value of a class.
- A stricter form that scopes `x` to the success branch is Open (§17).

### 7.6 Bugs stop the program (Decided)
With no exceptions, failures that are bugs rather than expected outcomes stop the program:
- integer overflow, `x / 0`, `x % 0`, and `MIN / -1`;
- an out-of-range `a[i]`;
- a cast that doesn't fit, such as `(u8) n` when `n` is 300 (§9.1);
- stack overflow (each stack ends in a guard page);
- exceeding `#max_memory_mb` (§8.2).

The program prints the message and the source location to stderr, flushes and closes open files, and exits with status 1. An embedded script returns the failure to its host through the C API. Nothing can catch it.

The explicit alternatives remain for code that wants another outcome:
- `+%` and the other wrapping operators (§9.2);
- the wrapping and saturating casts `(u8%) n` and `(u8|) n`, and `u8.from(n)`, which fails with `status.RANGE` (§9.1);
- `a[%i]`, `a[|i]` and `getOr` (§4.10);
- `divOr`.

**Closed pipes**: writing to stdout after the reading end has closed, as in `zink run tool.zn | head`, ends the program quietly, the way standard Unix tools do. The shell sees the usual SIGPIPE status. Writing to other files and pipes fails with `status.BROKEN_PIPE` instead.

### 7.7 Named codes (Decided)
Status codes follow Linux's `errno` numbers, with Zinkite's own codes from 192 up and a program's own from 224 up. So an operating-system error passes through unchanged, and `status.message(code)` can use the system's own description (`strerror`) for it.

```
File f @err = File.open(path);
if (err) {
    errprintln("can't open ${path}: ${status.message(err)}");   // "No such file or directory"
    return err;                                                 // in u8 main: the exit status
}
```

| Range | Used for |
|---|---|
| 0 | `OK`: success |
| 1 | `FAILED`: a failure with no particular reason, written `#1` |
| 2–191 | Linux's `errno` numbers (2–133 are assigned today; the rest are kept for future ones) |
| 192–223 | Zinkite's own codes |
| 224–255 | a program's own codes |

- **1 is the one exception to `errno`.** It means `EPERM` there, but `#1` is the natural way to say "failed", and exit status 1 means general failure too. An `EPERM` from the operating system is therefore reported as `PERMISSION` (13, `EACCES`), whose description, "Permission denied", says the same thing.
- **Every `errno` code is available under its C name**, such as `status.ENOENT` or `status.EACCES`. The common ones also have readable names:

  | Name | Number | `errno` | Description |
  |---|---|---|---|
  | `NOT_FOUND` | 2 | `ENOENT` | No such file or directory |
  | `INTERRUPTED` | 4 | `EINTR` | Interrupted system call |
  | `IO` | 5 | `EIO` | Input/output error |
  | `BAD_HANDLE` | 9 | `EBADF` | Bad file descriptor: also a closed `File` (§6.4) |
  | `AGAIN` | 11 | `EAGAIN` | Resource temporarily unavailable |
  | `NO_MEMORY` | 12 | `ENOMEM` | Cannot allocate memory: also GPU memory running out (`STDLIB.md` §10.6) |
  | `PERMISSION` | 13 | `EACCES` | Permission denied |
  | `BUSY` | 16 | `EBUSY` | Device or resource busy |
  | `EXISTS` | 17 | `EEXIST` | File exists |
  | `CROSS_DEVICE` | 18 | `EXDEV` | Invalid cross-device link |
  | `NOT_DIRECTORY` | 20 | `ENOTDIR` | Not a directory |
  | `IS_DIRECTORY` | 21 | `EISDIR` | Is a directory |
  | `INVALID` | 22 | `EINVAL` | Invalid argument: also `i64.parse("abc")`, a value that isn't a code point, and malformed input to a parser |
  | `NOT_TERMINAL` | 25 | `ENOTTY` | Inappropriate ioctl for device |
  | `NO_SPACE` | 28 | `ENOSPC` | No space left on device |
  | `BROKEN_PIPE` | 32 | `EPIPE` | Broken pipe |
  | `RANGE` | 34 | `ERANGE` | Numerical result out of range: also `i64.parse` of a number too big for the type |
  | `NOT_EMPTY` | 39 | `ENOTEMPTY` | Directory not empty |
  | `UNSUPPORTED` | 95 | `EOPNOTSUPP` | Operation not supported: also input using a feature a parser leaves out |
  | `TIMEOUT` | 110 | `ETIMEDOUT` | Connection timed out |

- **Zinkite's own codes**, for failures that aren't operating-system errors:

  | Name | Number | Description |
  |---|---|---|
  | `EMPTY` | 192 | Nothing to take: a pop on an empty array |
  | `FULL` | 193 | No room: a push on a full capped array |
  | `END` | 194 | End of input, or of an iterator |
  | `MISSING` | 195 | No such key or element |
  | `POSITION` | 196 | A position out of range, such as a cursor moved past the end |
  | `TYPE` | 197 | Wrong kind of value: `asI64()` on a string in a parsed tree (`STDLIB.md` §9.2) |
  | `GPU` | 198 | A graphics API call failed (`STDLIB.md` §10.6) |
  | `OUT_OF_DATE` | 199 | A window's surface changed, so the swapchain must be rebuilt (`STDLIB.md` §10.6) |

- **The numbers are Linux's generic table**, which x86-64, AArch64 and RISC-V share. A port to a system with other numbers would translate its errors into these.
- **Exit statuses**: the value `u8 main` returns becomes the exit status. Common codes don't collide with the shell's special values (126, 127, and 129–192 for death by signals 1–64). Zinkite's own codes start at 192, which overlaps only signal 64, one that programs rarely receive. Scripts that check particular exit codes should still return fixed numbers of their own.
- **A program's own codes** (224–255) have no built-in description: `status.message` gives "Error 230" for them.
- One status can say why a call failed, but not carry details such as a file name. The caller writes the message, and it knows the path anyway.

---

## 8. Program structure: directives, packages and imports

### 8.1 Syntax roles
- **`#`** means "decided by the compiler" at the start of a line: settings directives and conditional compilation, resolved before code generation. After `return`, `#` writes the call's status (§7.2). The lexer tells them apart by position.
- **`@`** reads a call's status (§7.3). Annotations, which `@` was reserved for, are dropped; none had been defined (Decided).
- **Runtime operations are ordinary functions**, never sigils.
- **`$`** is used for string interpolation, and in array types to mark the chunked index (§4.1). **`%`** stays the modulo operator.

### 8.2 Program-wide settings (Decided)
Settings apply to the whole program and are written as directives at the top of the file that contains the entry point (`static main`, or a top-level `main` in a script file, §8.6):

```
#!/usr/bin/env zink
#max_memory_mb 256

public class Main {
    public static void main(string[.] args) { ... }
}
```

Rules:
- **Only in the file containing the entry point**, identified by its `main`, not by file name. Anywhere else they are a compile error, so libraries can't change program behavior.
- They come after the shebang and before any declaration.
- **Unknown directives are errors**, so a typo is never silently ignored.
- Each setting may appear once, after `#if` is resolved.
- A setting that doesn't apply produces a warning.
- **Precedence: host API > build flag (e.g. `--max_memory_mb=512`) > directive > default.**
- Embedded scripts usually have no `main`, so the host sets these through the C API. If a loaded script has its own directives, the host's settings win and the compiler warns.

Setting names:

| Directive | Values | Default | Meaning |
|---|---|---|---|
| `#max_memory_mb` | integer | none | Ceiling on memory committed to data stacks; exceeding it stops the program (§7.6). How it is enforced without a check on growth is Open (§17) |
| `#data_stack_gb` | integer | 64 | Address space reserved per data stack (not memory) |

### 8.3 `#if` (Decided)
```
#!/usr/bin/env zink
#if debug
#max_memory_mb 64
#endif

public class Main {
    public static void main(string[.] args) {
        #if debug
        println("args: ${args.len}");
        #endif
        ...
    }
}
```

- **Conditions use only compile-time values**: build flags (`debug`, `--define color`) and the target (`os(linux)`, `arch(aarch64)`).
- **No `#define` and no text substitution**, so `#if` can't turn into the C preprocessor.
- **Each branch must be complete statements or declarations, and both branches must parse.** Only the active branch is type-checked, so typos are caught while platform-specific APIs in the inactive branch don't cause errors (Swift's approach).
- `#if` may wrap settings directives, which gives per-build settings for free.
- The lexer accepts `#!` on line 1 so scripts can be run directly.

### 8.4 Packages and files (Decided)
Java's model: a package is a directory, and each file's `package` line must match its path.

```
src/
  com/example/app/Main.zn        package com.example.app;
  com/example/util/MyArray.zn    package com.example.util;
```

- **File order**: shebang, directives (entry file only, §8.2), `package`, imports, declarations.
- **One public top-level class, root, interface or enum per file**, named after the file: `MyArray.zn` holds `public class MyArray`. A file may also hold non-public declarations, visible only inside its package.
- **No `package` line** puts the file in the unnamed package. That is meant for single-file scripts; as in Java, named packages can't import from it.
- **The source root is inferred from the entry file**: `zink run src/com/example/app/Main.zn` containing `package com.example.app;` gives the root `src/`. A `package` line that doesn't match the path is an error. Extra roots for libraries come from a `--source-path` flag.
- **Visibility**: `public`, `private`, and package-private when nothing is written (§6.1).
- **`vendor.*` is reserved** for bindings to third-party C libraries that ship alongside the standard library, such as GLFW, SDL3, OpenGL and Vulkan (`STDLIB.md` §10.6).
- **`zink.*` is reserved** for the standard library. `zink.lang` is imported into every file automatically (`print`, `println`, `errprint`, `errprintln`, the standard streams and the other built-ins). `STDLIB.md` lists every package and its contents.
- **Circular dependencies between packages are allowed.** The compiler sees the whole program at once, which monomorphization needs anyway (§10).

### 8.5 Imports (Decided)
```
package com.example.app;

import com.example.util.MyArray;              // usable as MyArray
import com.example.MyArray as ExampleArray;   // usable as ExampleArray only
import com.example.io.*;                      // every public declaration in com.example.io

public class Main {
    public static void main(string[.] args) {
        MyArray a = new MyArray();
        ExampleArray b = new ExampleArray();
        com.example.other.Thing t = new com.example.other.Thing();   // full names always work
    }
}
```

Rules:
- **`as` renames a declaration for this file only.** The original simple name isn't imported: after `import com.example.MyArray as ExampleArray;`, a bare `MyArray` means whatever it would mean without that line.
- **Two imports can't bring in the same name.** `import com.example.util.MyArray;` together with `import com.example.MyArray;` is an error, and the message suggests `as`. An import that clashes with a declaration in the same file is also an error. A local variable or parameter hides any class with the same name, imported or declared, inside its scope, as in Java: with `string path` in scope, `path.join(...)` means the variable's method (Decided). Names that come automatically from `zink.lang` are different too: a declaration of the program's own hides the `zink.lang` one wherever it is visible, as a class of your own hides `java.lang`'s in Java (Decided).
- **Wildcards work as in Java**: an explicit import or alias wins over a wildcard, and a name provided by two wildcards is an error only where it's used.
- **Only public declarations can be imported** from another package.
- **Unused imports** are a warning.
- Static imports and aliases for whole packages: Open (§17).

### 8.6 Script files (Decided)
A file doesn't need a class. A file whose top level has methods and fields but no class declaration is a **script file**, as in Java 25's compact source files:

```
#!/usr/bin/env zink
#max_memory_mb 64

i64 total = 0;

void main(string[.] args) {
    total = args.len;
    println("got ${total} arguments");
}
```

- **The compiler wraps the file in a hidden class.** Its top-level methods and fields become `static` members of that class, so they work exactly like static members everywhere else.
- **Entry point**: `main()` or `main(string[.] args)` at the top level, returning `void` or `u8`. Directives (§8.2) go at the top of the file as usual.
- **Exit status** (Decided):
  - `u8 main(...)` returns the status, and falling off its end returns 0, as in C;
  - `void main(...)` always exits with 0;
  - the status is a `u8` because Linux keeps only its low 8 bits. With a wider type, `return 256;` would reach the shell as 0, which means success;
  - `main` can't set a status: `return #1;` in `main` is an error that suggests `return 1;`;
  - `exit(code)` ends the program from anywhere with that status. Open files are flushed and closed (§6.4), but `defer`s don't run, as in Goose;
  - a program that stops on a bug exits with 1 (§7.6).
- **A script file can also declare classes and interfaces**, which are visible only inside that file.
- **It is in the unnamed package and has no `package` line.** It can import other packages, but nothing can import it. Once code is worth sharing, it moves into a class in a package.
- Internally everything is still a class, so the type checker, the layout rules and the backends need no special case.

### 8.7 Static initialization (Decided)
- **Every static field is initialized before `main` runs**, once. For an embedded script, this happens when the host loads it, before its first call.
- **The order follows dependencies.** If the initializer of `A.x` reads `B.y`, directly or through any function it calls, `B.y` is initialized first. The compiler computes this order over the whole program. Independent fields keep declaration order, and unrelated files follow a fixed order, so every run is the same.
- **Initializers that depend on each other in a loop are a compile error** that lists the loop.
- **No lazy initialization on first use.** Startup cost is predictable, and dead-code elimination drops statics that are never used.
- **If a static initializer stops the program** (§7.6), it stops before `main`. For an embedded script, the load call fails.
- **Static values live for the whole run.** A growable static gets its own data stack. Nothing runs for them at exit, except that the runtime flushes and closes any files still open (§6.4).

---

## 9. Types and syntax

### 9.1 Number types and sizes (Decided)

| Type | Size | Meaning |
|---|---|---|
| `i8` `i16` `i32` `i64` `i128` | 1, 2, 4, 8, 16 | signed integers |
| `u8` `u16` `u32` `u64` `u128` | 1, 2, 4, 8, 16 | unsigned integers |
| `f32` `f64` | 4, 8 | floating point |
| `bool` | 1 | |
| class values | the sum of their fields, plus alignment (§11) | |
| `T&` | 8 | reference |
| interface references (`Shape&`) | 16 | |
| slices `T[:]`, `string` | 16 | |

- **One spelling per type.** There are no aliases: `long`, `short`, `byte` and `double` are compile errors that suggest the new name, since Java programmers will type them out of habit. `int` and `float` name generic bounds (§10.1), not types, so `int x = 5;` is an error too, and suggests `i64`.
- **Lengths, indexes and cursor positions are `i64`.** Indexes are signed, so a computed index that goes negative is out of range instead of wrapping (§4.10).
- **Literals take their type from where they are used**: `u8 b = 200;` is a `u8`, and `f64 x = 0.5;` an `f64`. With nothing to go by, an integer literal is an `i64`, the type of lengths and indexes, and a floating-point literal an `f64` (Decided). So `var i = 0;` is an `i64`, as are `"${1 << 40}"` and `max(3, 5)`. A suffix gives another type where it's needed: `255u8`, `1.5f32` (Decided, as in Rust). A literal that doesn't fit its type is a compile error.
  - **The type flows into arithmetic** (Decided). In `i64 big = 100000 * 100000;`, both literals are `i64`, so the multiplication is done in 64 bits instead of overflowing. In `a.len * 2`, the `2` takes the type of the other operand.
- **Conversions that can't lose information are implicit** (Decided): to a type that holds every value of the source. That is a wider integer type of the same signedness (`i8` to `i32`), a wider signed type for an unsigned one (`u8` to `i32`, `u32` to `i64`), `f32` for integers of up to 16 bits, and `f64` for integers of up to 32 bits and for `f32`. Anything else is a cast. Arithmetic on two different types follows the same rule (§9.2). An array also converts to a slice of its elements (§4.10).
- **`i128` and `u128`** (Decided) are for what 64 bits can't hold: the full product of two `u64`s (`(u128) a * b`, one instruction), 128-bit hashes and IDs, IPv6 addresses, and sums that would overflow. They follow every rule above, and the widening from 64 bits is implicit.
  - **A literal above `i128.MAX` is a `u128`**, as only a `u128` holds it: `u128 mask = 0xffff_ffff_ffff_ffff_ffff_ffff_ffff_ffff;`. Elsewhere it doesn't fit, a compile error. `-170141183460469231731687303715884105728` is `i128.MIN`, though its digits alone don't fit an `i128`.
  - **They map to the C compilers' 128-bit integers** (`__int128`, in GCC and Clang on 64-bit targets), so arithmetic and its overflow checks cost about two 64-bit operations. They're 16-byte aligned.
  - **There are no 256-bit types.** C compilers don't have them yet (C23's `_BitInt` would), and the main use, cryptography, needs constant-time code that checked arithmetic isn't. `BigInt` (`STDLIB.md` §10.4) covers big numbers.
- **`null` applies only to nullable references** (`T&?`): `i32 x = null` is a compile error.
- **Casts exist only for numbers.** Interface references use `instanceof` (§6.3).
- **A cast never changes a value silently** (Decided). With `n` an `i64`:
  ```
  (u8) n                     // stops the program when n doesn't fit, as with n = 300 or n = -5 (§7.6)
  (u8%) n                    // wraps, keeping the low 8 bits: 300 gives 44, and -5 gives 251
  (u8|) n                    // saturates: 300 gives 255, and -5 gives 0
  u8 b @bad = u8.from(n);    // reports instead: 0, with status.RANGE
  ```
  - The marks are the ones the wrapping and saturating operators (§9.2) and the index modes (§4.10) use.
  - `u8.from(n)` is for values from outside the program, such as a port number read from a configuration file (`STDLIB.md` §2.4).
  - **A constant that doesn't fit is a compile error**: `(u8) 300` doesn't compile, while `(u8%) 300` is 44.
  - **The check is one compare and branch**, and the compiler drops it when it can prove the value fits: `(u8) (x & 0xFF)`, `(u8) (h >> 56)` for a `u64` `h`, or inside `if (x < 256)`.
  - **Remainders keep their sign**: `(u8) (i % 256)` stops when `i` is negative, since `-1 % 256` is -1. `(u8%) i` takes the low byte of any `i`.
  - **Between signed and unsigned types of the same size**, only `%` reinterprets the bits: with `x` = -1, `(u64%) x` is `u64.MAX`, and `(u64) x` stops.
- **Float to integer** truncates toward zero: `(i64) 3.7` is 3, and `(i64) -3.7` is -3. NaN or a value out of range stops the program, `(u8|) x` saturates with NaN giving 0, and `%` is a compile error. To round instead, use `math.round` first (`STDLIB.md` §8.1).
- **To a float, nothing stops.** An integer rounds to the nearest value (`(f64) i64.MAX` is 2⁶³), and `(f32) x` rounds to the nearest `f32`, giving an infinity beyond its range, as IEEE 754 arithmetic does. `%` and `|` are compile errors with a float target.
- **No `char` type** (Appendix A.10). A Unicode code point is a `u32`. A character literal is an integer literal whose value is the code point: `'a'` is 97, `'\u{1F600}'` is 0x1F600.
  - Like other literals, it takes its type from where it's used, and is a `u32` with nothing to go by.
  - **Used as a `u8`, it must be ASCII** (below 128). `'é'` is code point 233, which fits in a byte, but in UTF-8 text é is two bytes, so `line[i] == 'é'` would compile and never match. It is a compile error instead.
  - It must fit its type: `u16 c = '\u{1F600}';` is a compile error.
  - Whether a `u32` is a real code point (not a surrogate, at most 0x10FFFF) is checked when it is turned into text (§4.13).
- **No integers narrower than a byte** (`i4`, `u4`) for now. A value can't be addressed below a byte, so they would only save space when packed into classes and arrays, and those can't hand out references (Open, §17).

### 9.2 Integer arithmetic
- **Integer overflow stops the program** (§7.6), for signed and unsigned types alike. That covers `+`, `-`, `*`, unary `-`, `++` and `--` when the result doesn't fit the type (so `0u32 - 1` stops), and also `x / 0`, `x % 0` and `MIN / -1`. `MIN % -1` is 0, its mathematical value. The check is one branch on the CPU's overflow flag, as in Swift.
- **Bitwise operators and shifts never fail.** `>>` is an arithmetic shift on signed types and a logical shift on unsigned ones, so Java's `>>>` isn't needed and is gone.
- **Wrapping is written explicitly**: `+%`, `-%`, `*%`, and compound forms such as `+%=`.
- **`a.divOr(b, 0)`** gives 0 instead of dividing by zero.
- **Saturating is written explicitly too**: `+|`, `-|`, `*|`, and compound forms such as `+|=`. The cursor operators use `^` (§4.8), so the two don't clash.
- **Mixing integer types** (Decided). An arithmetic or bitwise operator on two different integer types works when one type holds every value of the other (§9.1), and the result has that type. Otherwise it's a compile error that suggests a cast. A third, wider type is never chosen.
  ```
  // b: u8, n: i32, c: u32, d: i32, h: u64, len: i64
  n + b          // an i32: every u8 fits in an i32
  len - c        // an i64
  c + d          // error: neither u32 nor i32 holds every value of the other. Write (i64) c + d
  h % len        // error: write h % (u64) len
  ```
  - So a result always has one of its operands' types, and `u64` with `i64` gets the same answer as `u32` with `i32`, though an `i128` would hold both.
  - Literals take the other operand's type (§9.1), so `c - 1` and `b + 1` need nothing.
  - Integers with floats follow the same rule: `n * x` with an `i32` `n` and an `f64` `x` is an `f64`, while an `i64` `n` needs `(f64) n`.
  - `x += y` needs `y` to convert implicitly to the type of `x`, so `x += len` with an `i32` `x` is an error.
- **Comparisons between any two integer types are exact** (Decided), `==` and `!=` included. With `d` = -1 and `c` = 0, `d < c` is true; in C it's false, because -1 becomes `UINT_MAX`. When neither type holds the other (`u32` with `i32`, `u64` with `i64`), it costs one extra test, as C++20's `std::cmp_less` does.
- **A shift count can be any integer type.** The result has the type of the left operand.

### 9.3 Function types and lambdas (Decided)
```
func(i32, i32 => i32) add = (i32 a, i32 b) => a + b;
i32 x = add(2, 3);

var adults = dogs.filter((Dog d) => d.age >= minAge);
rows.sort((Row a, Row b) => { return a.num < b.num; });     // a block body
```

- **Lambda parameters always have their types written** (Decided): `(i32 a, i32 b) => a + b`, never `(a, b) => a + b` or `d => d.age`. A lambda then reads the same wherever it appears, needs no type from where it is passed, and lets overloads be chosen from argument types alone (§6.5).
  - The parentheses are always written, even for one parameter: `(Dog d) => d.age`.
  - `var` isn't allowed as a lambda parameter's type.
  - The result type comes from the body: the expression after `=>`, or the `return`s of a block.
  - The parameter types must be exactly those of the `func` type the lambda is passed as, with no implicit conversions. So `bytes.sort((i64 a, i64 b) => a < b)` on a `u8` array is an error that names `u8`.
  - An unused parameter is named `_`: `(i64 a, i64 _) => a`.

- **`=>` replaces the previous `->`**, which is now the pop operator (§4.8). C# uses `=>` the same way.
- **`func(A, B => R)`** is the type of a function that takes an `A` and a `B` and returns an `R`. Both sides are always written, with `void` for "none".
- **Lambdas passed as parameters are compile-time values**, as in Goose. `filter` is compiled with the lambda inlined, so it costs what the handwritten loop would, and the lambda may capture anything in scope.
- **A `func` stored in a variable, field or array can't capture.** It is a plain function pointer. Stored closures that capture are Open (§17).
- **Captures are read-only**: a captured variable must never be assigned after its initialization (Java's "effectively final" rule).
- Method references such as `this::close`: Open.

### 9.4 Statements and expressions (Decided)
Statements and expressions follow Java unless listed here: operators and precedence, `if`/`while`/`do`/`for`, `break`/`continue`, `?:` and casts. There are no `try`, `catch`, `finally` or `throw`.

```
var total = 0;                          // type inferred from the initializer: an i64

for (var i = 0; i < 100; i++) {         // the classic for loop; i is an i64
    total += i;
}

for (u32 x : myArray) { ... }           // for-each, copying each element
for (Item& it : items) {                // for-each, by reference
    it.heavier(1.0);
}
for (u32 c : line.codePoints()) { ... } // for-each over an iterator: no array is built
while (Pending p : todo ->) { ... }     // a repeat loop: pops before every pass, until the stack is empty

string size = switch (count) {          // arrow form only, no fall-through
    case 0 => "none";
    case 1, 2 => "a few";
    default => "many";
};

enum Color { RED, GREEN, BLUE }
```

- **`var`** works only for local variables with an initializer, and copies (§5.1).
- **The classic `for`** is Java's: `for (init; condition; step)`. `for (var i = 0; i < n; i++)` makes `i` an `i64`, the default type of an integer literal (§9.1).
- **Unsigned integers work as conditions** (Decided). In `if`, `while`, `do`, the condition of `for` and `?:`, and as operands of `!`, `&&` and `||`, a `u8`, `u16`, `u32` or `u64` counts as false when it is 0 and true otherwise. So `if (err)` tests a status (§7.3).
  - Signed integers and floats don't work this way: write `n != 0`.
  - There's no conversion outside conditions: `bool b = err;` is an error; write `err != 0`.
  - An assignment can't be used directly as a condition: `if (x = 0)` is an error, since with integers as conditions it is usually a typo for `==`.
- **For-each** is Java's too, and works over:
  - every array kind and slice, in logical order;
  - a `hashmap` or a `hashset`, in insertion order (§4.14), and a `pool`, over its live slots (`STDLIB.md` §7.5);
  - an **iterator**: any value with a `mut T next()` method that returns a nonzero status when nothing is left (Decided);
  - any value whose class has an `iterator()` method returning an iterator, as with Java's `Iterable`: `for (Node& n : tree)`.

  `for (Item it : items)` copies each element; `for (Item& it : items)` refers to it in place. With an iterator, that takes a `mut T& next()`.
  - Appending to an array inside its own loop is safe: elements never move (§3.1), and the loop re-reads `len` on each step. Removing from the array inside a by-reference loop is a compile error (§5.3), and inside a by-value loop it is too, until §17 decides otherwise.
- **Repeat loops** (Decided): `while (T x : call)` makes the call before every pass. While the call's status is 0, its value is bound to `x` and the body runs. When the status is nonzero, the loop ends without running the body.
  ```
  while (Pending p : todo ->) { ... }                    // pops until the stack is empty
  while (DirEntry e : d.next(names)) { ... }             // a next() that takes an argument
  while (WalkEntry e : w.next()) {
      if (e.isDir && e.name == ".git") { w.skip(); continue; }   // steers the walker it names
      ...
  }
  while (Step s : queue.heapPop((Step a, Step b) => a.dist < b.dist)) { ... }
  while (Task& t : sched.nextReady()) { t.run(); }       // by reference, from a mut T& method
  while ((string key, string value) : ini.next()) { ... }   // a tuple result, destructured
  while (string line : stdin.nextLine(buf)) { ... }      // STDLIB.md §4.1
  ```
  - **`for` evaluates its source once; `while` calls again on every pass**, as it re-tests a condition. The call is written with its parentheses because it really is made each time, with its arguments evaluated each time.
  - **The source is anything `@` can read** (§7.3): a call, an array operator such as `todo ->`, or a parenthesized one. The loop ends exactly when `@` would give a nonzero status.
  - **The call must be able to fail.** One that never sets a status is a compile error that suggests `while (true)`, so `while (var it : s.lines())` can't loop forever by accident.
  - **The header declares new variables only**: a type or `var` (with `&` for a reference), a tuple of them, or `_`. They belong to the body, and each pass declares them afresh, so a call may remove from what the previous pass's value viewed (`REFERENCES.md` §9).
  - **`continue` makes the next call; `break` leaves without one.**
  - **A method that returns `void`** has nothing to bind, so it takes a plain condition: `while (!@r.next(fields, strings))`.
  - **The status that ended the loop** (Decided) can be captured after the header. It is declared in the enclosing block, since it is read after the loop:
    ```
    while (JsonEvent e : r.next()) @why { ... }
    if (why != status.END) { ... }        // an error stopped the loop, not the end of the input
    ```
    `why` is the nonzero status of the last call, or 0 when a `break` ended the loop. With a capture, the body must be a block.
- **Iterators** (Decided):
  ```
  class Countdown {
      i64 n;
      mut i64 next() {
          if (n == 0) { return #status.END 0; }   // nothing left, with the zero value
          n--;
          return n + 1;
      }
  }

  for (i64 x : new Countdown(3)) { println("${x}"); }    // 3, 2, 1
  ```
  - **A for-each over an iterator is a repeat loop on a hidden local**: `for (T x : e)` means `var it = e;` followed by `while (T x : it.next())`. So `break`, `continue` and `return` work as in any loop.
  - **For-each over an iterator variable is a compile error** (Decided) that suggests the repeat loop: `for (string l : lines)` becomes `while (string l : lines.next())`. A for-each then never changes a variable the program can see, and code that steers an iterator or reads it afterwards names it in the call.
  - **No runtime cost**: the iterator's class is known at compile time, so `next()` is a direct call that gets inlined. The loop compiles to the same code as a hand-written one.
  - **Iterators are lazy**, so iterating doesn't build an array first.
  - **The usual reference rules apply**: an iterator over `text` holds a view into it, so removing from `text` inside the loop is a compile error (`REFERENCES.md` §9).
  - **For generic code**, `zink.lang` declares `interface Iterator<T> { mut T next(); }`, so a function can take `<I implements Iterator<T>>`.
  - **Library iterators**: `s.codePoints()`, `s.lines()` and `s.splitBy(sep)`. The eager `s.split(sep)` still returns a `string[..>]` when an array is wanted.
  - Goose has no iterator protocol; it iterates through functions that take a lambda. Those stay available in the library too (`filter`, `map`, `sort`, `retain`).
- **`switch`** has only the arrow form: no fall-through and no `break`. The case arrow is `=>`, as for lambdas (§9.3). A case can list several values (`case 1, 2 =>`). It works on integer types (character literals included), `string` and enums, as a statement or as an expression. As an expression it must cover every case, with `default` or every enum constant.
- **`yield`** gives the value of a `switch` expression from a block arm, and a **label** names a loop for `break` and `continue`: `outer: for (...) { ... break outer; }` (`GRAMMAR.md` §5).
- **Enums** are Java-style lists of constants, stored in static data. Enums with fields, and Goose-style variants with payloads, are Open (§17).

### 9.5 Multiple results (Decided)
```
(string, string) splitFirst(string s) {
    for (i64 i = 0; i < s.len; i++) {
        if (s[i] == ' ') { return (s[:i], s[i + 1:]); }
    }
    return (s, "");
}

(string cmd, string arg) = splitFirst(line);      // declares cmd and arg
(cmd, arg) = splitFirst(other);                   // assigns existing variables
(string word, _) = splitFirst(line);              // _ discards one result
(i64 q, i64 r) @err = divMod(a, b);                // with the status, which stays a u8
(x, y) = (y, x);                                   // a swap: the right side is evaluated first
```

- **Declaring them**: a function returns several values by declaring a tuple type, `(T1, T2, ...)`, with two or more types, and writing `return (e1, e2, ...);`. The status works as usual: `return #status.INVALID (0, 0);`, and each result is its zero value on failure (§7.5).
- **Tuples aren't values in their own right.** A call that returns one must be destructured, discarded with `_ = f();`, or called as a plain statement. There are no tuple variables, fields or array elements, and no `t.0` (Open, §17).
- **Destructuring** either declares every target, with a type or `var`, or assigns to existing variables. `_` works in both forms, and a status capture goes after the closing parenthesis.
- **Tuple expressions** appear only in a `return`, as a lambda's result, and on the right of a destructuring.
- **Function types** can return tuples too: `func(string => (string, string))`.
- **No extra cost**: results come back in registers, or are built in place in the caller's variables (§3.3), as for a single result. They cost the same as a small class with the same fields, and less than scanning twice.
- **Each result has its own roots** (`REFERENCES.md` §4): both results of `splitFirst` are rooted at its argument.

---

## 10. Generics
- **Everything is monomorphized.** Values of different classes have different sizes, so each instantiation is compiled separately with sizes and offsets hardcoded. Goose does the same.
- **Angle brackets** (`hashmap<K, V>`) are kept (Decided). Slot types inside them (`hashmap<string, u8[..>16]>`) work because `[...]` in a type is lexed on its own (§4.12).
- **Array parameters are specialized per layout** (§4.3).
- **Compile-time constants as type arguments** are needed for capped arrays (`T[.N]`); the syntax is Open (§17).
- No runtime type information.
- **Compiler details**:
  - Instantiations get internal names users can't type, so they never collide with user declarations. Errors show the written form.
  - Nested instantiations are cached by generic and type arguments.
  - For recursive types, the instance is registered in the cache before its body is expanded.
  - Runaway instantiation (`Foo<T>` using `Foo<Pair<T, T>>`) is stopped by a depth limit, or rejected.
  - Only what is used is instantiated. The compiler sees the whole program (no dynamic loading).
  - Generic bodies are type-checked once, against their declared bounds.
- **Type arguments are inferred from the arguments** (Decided). Each parameter whose type mentions a type parameter fixes it from its argument, and an array passed where a slice (`T[:]` or `T[:]&`) is expected gives its element type: `summarize(ints)` with an `i64[..>]` gives `T = i64`. A literal takes the type the other arguments fix. Arguments that fix one type parameter differently, such as `max(a, b)` with a `u8` and a `u32`, are an error; no conversion is applied to make them agree. Explicit type arguments (`f<i64>(x)`) are Open (`GRAMMAR.md` §8).

### 10.1 Built-in bounds (Decided)
A generic body can use only what its bounds allow (§10). Seven bounds for the built-in operations are built in. They are used with `implements`, like interfaces, and `&` combines several: `<T implements ordered & hashable>`.

| Bound | Satisfied by | Allows, besides `==` |
|---|---|---|
| `sint` | `i8` `i16` `i32` `i64` `i128` | what `int` allows, and negative literals |
| `uint` | `u8` `u16` `u32` `u64` `u128` | what `int` allows, and use as a condition (§9.4) |
| `int` | every `sint` and `uint` | what `number` allows, plus `%`, the wrapping and saturating operators, bitwise operators and shifts, `T.MIN`, `T.MAX` and the bit methods (`STDLIB.md` §8.1) |
| `float` | `f32` `f64` | what `number` allows, plus `T.NAN`, `T.INFINITY`, `T.EPSILON`, `isNaN()`, `isFinite()` and the `math` functions |
| `number` | every `int` and `float` | what `ordered` allows, plus `+`, `-`, `*`, `/`, unary `-`, casts between `T` and any number type (`(T) x`, `(f64) t`), `T.from(x)` and `T.parse(s)` |
| `ordered` | numbers, `bool`, `string`, enums, and classes that implement it | `<`, `<=`, `>`, `>=` and `compareTo` |
| `hashable` | integers, `bool`, `string`, enums, and classes whose fields are all `hashable` | use as a `hashmap` key or a `hashset` element |

```
<T implements number> T sum(T[:] xs) {
    T total = 0;
    for (T x : xs) { total += x; }
    return total;
}

<T implements ordered> T clamp(T x, T lo, T hi) { return x < lo ? lo : (hi < x ? hi : x); }

<K implements hashable> i64 countDistinct(K[:] xs) {
    hashset<K> seen = [];
    for (K x : xs) { seen.add(x); }
    return seen.len;
}
```

- **Bounds aren't types.** `ordered x;`, `number&` and `int[..>]` are compile errors, and `int` and `float` used as types suggest `i64` and `f64` (§9.1). The names are built in, like `hashmap`: they aren't keywords, but no declaration may reuse them.
- **Which bounds include which**: `sint` and `uint` are `int`; `int` and `float` are `number`; every `number` is `ordered`, and every `int` is `hashable`. So a value of a `<T implements int>` parameter can be passed where `ordered` or `hashable` is needed.
- **Only built-in types satisfy the number bounds.** A class can't implement `sint`, `uint`, `int`, `float` or `number`, so user classes get no arithmetic operators.
- **A class implements `ordered` with one method**, `i32 compareTo(C other)`, which returns below 0, 0 or above 0, as `string.compareTo` does. Then `<`, `<=`, `>` and `>=` work on its values everywhere, not only in generic code:
  ```
  class Version implements ordered {
      u32 major; u32 minor; u32 patch;
      i32 compareTo(Version o) {
          if (major != o.major) { return major < o.major ? -1 : 1; }
          if (minor != o.minor) { return minor < o.minor ? -1 : 1; }
          if (patch != o.patch) { return patch < o.patch ? -1 : 1; }
          return 0;
      }
  }

  if (installed < required) { ... }
  Version newest @none = versions.max();       // STDLIB.md §7.1
  ```
- **`hashable` is worked out from the fields** (§4.14). A class can't opt in or out. Floats aren't `hashable`, since NaN isn't equal to itself.
- **How built-in values are ordered**: numbers by value, `false` before `true`, strings byte by byte (so UTF-8 text sorts by code point), and enums in declaration order. So `a.word < b.word` compares two strings directly.
- **Floats are `ordered`, and `<` on them stays IEEE 754**: every comparison with NaN is false. `sort`, `min`, `max` and the binary searches (`STDLIB.md` §7.1) use a total order instead, which puts NaN last, so a NaN can't break a sort.
- **A literal in a generic body must fit every type its bound allows**: 0 to 127 under `number` and `int`, -128 to 127 under `sint`, 0 to 255 under `uint`, and anything that fits an `f32` under `float`. So `T total = 0;` and `x * 2` compile, and `T k = 1000;` doesn't. `(T) 1000` does compile, and is an error only in the instances where 1000 doesn't fit (here `i8` and `u8`).
- **`(T) x` is a cast** when `T` is a type parameter in scope (`GRAMMAR.md` §6).

---

## 11. Memory layout (Decided)
- **Class values have no header.** Fixed-size fields come first, sorted by size (largest first), then by name, so they pack with almost no padding and always sit at fixed offsets. Variable-size fields follow, in declaration order.
- **Arrays** are `[metadata][elements]`, inline and packed, never a pointer to a separate block. The width of the metadata comes from the cap, or from a varint (§4.6).
- **A root is the one exception**: its record holds where each growable part lives, on that part's own data stack (§6.6).
- **Alignment**: a fixed-size value is aligned to its largest field. Variable-size records are packed; reads use unaligned loads, which x86-64 and AArch64 support.
- **Data stacks**: each is an address-space reservation (`#data_stack_gb`, 64 GB by default) ending in an unmapped guard page. The compiler assigns stacks statically, and a scope saves and restores the tops of the stacks it uses.
- **Static data** holds literals, statics, enum constants and interface method tables.
- **References** are plain 8-byte pointers. Interface references add a method-table pointer, and slices add a length.

---

## 12. Native compilation and runtime

### 12.1 Target
Programs compile to native machine code, through C (§12.5), so standalone CLI tools carry no VM or interpreter. Types choose instructions and then disappear, except for interface method tables.

### 12.2 The runtime is a library
A small C static library linked into every program. It holds:
- the data-stack reservations;
- built-ins: formatting, I/O, and the standard library's core;
- the host API.

There is no allocator, no counting and no collector. With dead-code elimination and a static build it is tens of KB.

### 12.3 Stacks and failures
- **No unwinding.** There are no exceptions: failures are status codes (§7), and bugs stop the program (§7.6). No unwind tables, landing pads or per-call error checks are needed.
- **Stack overflow**: the call stack and every data stack end in an unmapped guard page. Touching one stops the program, through a signal handler running on a separate small stack. Functions with frames larger than a page touch each page in order (stack probes), so a large frame can't jump past the guard page.

### 12.4 Dispatch (Decided)
- Direct calls for everything whose type is known, which is every call except through an interface reference.
- One method table per (class, interface) pair, in static data. An interface call loads the function pointer from the reference's table.

### 12.5 Backends and executables (Decided)
- **The compiler emits C**: one C file for the whole program, as Goose does. A C compiler then turns it into native code.
  - `zink run` compiles that C with the system's C compiler at `-O0`, and caches the executable (§14.4).
  - `zink build` uses the same compiler at `-O2`, so the executable gets its full optimizer. Which compiler is used: §14.2.
- **Why C rather than machine code.** This reverses the earlier plan, to emit machine code through Cranelift, on 2026-11-19:
  - programs run faster, since gcc and clang optimize better than Cranelift, which trades code quality for compile speed;
  - the compiler is far less work: no instruction selection, register allocation, ELF writer or linker;
  - it reaches every platform with a C compiler, instead of one CPU at a time;
  - calling C becomes an ordinary C call (§17).
- **The costs**: running or building needs a C compiler installed, which most Linux systems have, and the first run after an edit pays for compiling, about a quarter of a second for a small program (measured on 2026-11-20). `zink run` code at `-O0` is slower than `-O2` in hot loops, so `zink run -O` builds at `-O2`.
- **A built-in TinyCC for `zink run` is a maybe-later** (§17), moved there from Decided on 2026-11-20. It would make the first run faster and need no installed compiler, but it lacks what the generated C relies on.
- **Calling convention**: the platform's C ABI. The status rides in a small returned struct, `{value, status}`, which C compilers return in two registers on Linux x86-64 and AArch64. Large results are built in place through a destination pointer (§3.3).
- **Debug info** comes from the C compiler's `-g`, with `#line` directives pointing debuggers and messages at the `.zn` source.
- **A direct machine-code backend** stays possible later, over the same typed IR, if depending on a C compiler ever becomes a problem.

### 12.6 Calling C (Decided)
```
extern import stdio.h;                         // a C header, as #include <stdio.h>
extern import SDL3/SDL.h;
extern link SDL3;                              // linked as -lSDL3

extern("write") i64 writeBytes(i32 fd, u8[:] data, u64 n);   // what the header can't say

FILE f = fopen("out.txt", "w");                // C functions are called like any other
if (f == null) { return #extern.errno(); }     // and fail as C does: errno is the status
fputs("hello\n", f);
fclose(f);
writeBytes(1, msg, (u64) msg.len);
```

- **`extern import` reads a C header**, named as in `#include <...>`, without quotes. Its functions are called like Zinkite functions, and its integer macros (`EOF`, `SEEK_SET`) and enum constants are names, wherever the C signature has one clear meaning:
  - numbers and `_Bool`, as the Zinkite type of the same size and signedness (`int` is `i32`, `size_t` is `u64`, `double` is `f64`, a C enum is `i32`);
  - `const char *` as `string`, copied with a NUL onto the scratch stack for the call (`STDLIB.md` §10.6);
  - a pointer to a C struct as a **handle** (below).
- **The rest needs an explicit declaration**: any other pointer, which could be one value, an array or something C keeps; function pointers; structs by value; and variable arguments, as `printf`'s. Calling such a function is an error that says why and suggests the declaration.
- **An explicit declaration** is `extern` on a body-less static function, with the C name in parentheses when it differs from the Zinkite one, as the bindings' namespaces need (`STDLIB.md` §10.6). What may cross:
  - parameters: numbers, `bool`, `string` (copied with a NUL), slices of numbers, which pass their pointer only, so a length is its own parameter, writable views (`T[:]&`), which C may write through, and handles;
  - results: numbers, `bool`, handles and `void`.
- **Handles**: a C struct used through pointers in an imported header, as `FILE` or `SDL_Window`, is a type of its own. It is a value that can be stored, compared, set to and compared with `null`, and passed back to C, but never looked into, so no raw pointer reaches Zinkite code (`STDLIB.md` §10.6).
- **C failures are C's**: a C function never fails by itself, and returns what C returns. `extern.errno()` reads `errno` as a status, since Zinkite's status codes follow errno's (§7.7).
- **Zinkite names win**: a C function with the name of a Zinkite function or built-in, such as `abs`, is reached through an explicit declaration under another name.
- **Reading headers needs clang**, which lists what a header declares; the program itself is then built with whichever C compiler §14.2 finds, which includes the headers as written.
- **The C side isn't checked.** A declaration that gets a pointer's meaning wrong, or C that keeps a pointer past the call, can break the reference rules. Explicit declarations belong in library and binding packages, which wrap them in checked Zinkite APIs (`STDLIB.md` §10.6).
- **Zinkite's output goes through C's `stdout`** (Decided), so text printed by Zinkite and by C code it calls comes out in the order it was written, on a terminal and through pipes alike. stdio buffers it by lines on a terminal and in 64 KB blocks otherwise; stderr is unbuffered, and stdout is flushed before anything goes to stderr. On glibc, Zinkite writes into the block buffer directly, as glibc's own `putc_unlocked` does, so output costs what a private buffer would.

---

## 13. Embedding in other programs (Open)
How a host program would run Zinkite code is Open. The options:
- **TinyCC inside the host**: the host links `libtcc` and the runtime, and compiles scripts when it loads them. Loading is fast; the code is unoptimized.
- **Precompiled plugins**: `zink build` produces a `.so` that the host loads through a C ABI. The code is fully optimized, but scripts must be compiled ahead of time.
- **A bytecode VM** over the same typed IR: typed opcodes (`ADD_I32`, `ADD_F64`, `LOAD_I64`) with no runtime type checks, and data stacks as in native code. It needs a dispatch loop, a bytecode format, a loader and, for untrusted scripts, verification, and it runs roughly 10–50× slower than native code.
- **None**: CLI tools don't need embedding.

Whichever is chosen, the host sets program settings through the API (precedence in §8.2), and the open question of references a host holds into a script's data applies (`REFERENCES.md` §12).

---

## 14. Compiler implementation and build order

### 14.1 Rust for the compiler, C for the runtime (Decided)
**Rust for the compiler**
- Compilers are mostly tree and graph transformations. Enums with exhaustive matching flag every place that fails to handle a new AST or IR node.
- Many passes (parsing, type checking, monomorphization, layout, root inference, IR lowering, codegen): the type system catches most cross-pass refactoring mistakes.
- Calling C from Rust is routine, and running the system C compiler is a process launch.
- Graph-shaped data is stored in arenas and referred to by integer index.
- Cargo, tests and fuzzing work out of the box.

**C for the runtime**
- Tiny, linked into every program, with a clean C ABI for embedding.
- Shipped as C source inside `zink` and compiled with the program's C, once per C compiler and set of flags, then cached. Users never see it.

### 14.2 What emitting C requires
- **C without undefined behaviour.** Checked arithmetic goes through `__builtin_add_overflow` and its siblings, and wrapping arithmetic (`+%`) through unsigned types. `-fwrapv` and `-fno-strict-aliasing` are safety nets, not the plan.
- **Names**: Zinkite identifiers are mangled into C identifiers that can't clash with C keywords, the C library or the runtime.
- **`#line` directives**, so C compiler messages and debuggers point at `.zn` lines.
- **The status** in a small returned struct, and large results built through a destination pointer (§12.5).
- **Finding the system C compiler** for `zink build` and `zink run -O`: `$CC`, then `clang`, then `gcc`, and `cc` as a last resort (Decided). Clang comes first because, measured on Zinkite programs, its code was as fast as gcc's or faster (by a quarter to a third on sorting and text) and it compiled faster at `-O0`. A missing one is an error that suggests installing clang or gcc.
- **Checking the generated C** under both clang and gcc, at `-O0` and `-O2`, since all four must give the same results (§18).

### 14.3 Build order (Decided)
1. **Write the reference rules** as a specification before any compiler code. They decide what is safe, and Goose's author credits this kind of spec work for much of that language's quality. Drafted: `REFERENCES.md`.
2. **Prototype the array layouts in C** and benchmark them (started: `bench/gap_buffer.c`, §15).
3. **Typed IR**: layouts, slot arrays, references with roots, the status channel; monomorphization done before the IR.
4. **Root inference and lifetime checks**: the hardest part of the compiler.
5. **C backend**: `zink run` works end to end, through the system C compiler.
6. **Optimized builds** through the system C compiler: `zink build`, and `zink run -O`.
7. **A direct machine-code backend** later, only if depending on a C compiler becomes a problem.

### 14.4 Command-line tool (Decided)
The compiler ships as one command, `zink`:

| Command | What it does |
|---|---|
| `zink build main.zn -o tool` | Compile to a native executable at the given path, with the system C compiler at `-O2` |
| `zink run main.zn [args...]` | Compile into the cache if needed, then run the result |
| `zink main.zn [args...]` | Same as `zink run`; this is what a `#!/usr/bin/env zink` shebang calls |
| `zink run -O main.zn [args...]` | The same, but built with the system C compiler at `-O2`, for scripts with hot loops |
| `zink cache clean` | Empty the cache |

Native code still needs a compile step, so `zink run` works like `go run` or `cargo run`, with a cache that makes repeat runs as fast as a prebuilt binary:

```
zink run main.zn arg1 arg2
  1. hash the sources + compiler version + target + build flags
  2. look in the cache for <hash>
       hit  -> go to 4
       miss -> generate C, and compile it with the system C compiler (at -O0, or -O2 with -O) to a native executable
  3. write it to a temporary file in the cache, then rename it into place
  4. exec it with arg1 arg2
```

Rules:
- **`exec`, not spawn.** The program replaces the `zink` process, so it keeps the process ID, stdin/stdout/stderr, signals and exit code. Pipes, `$?` and Ctrl-C behave as if the binary were run directly.
- **Arguments.** `zink`'s own flags go before the source file; everything after it goes to the program unchanged. `zink run -O main.zn -v` passes `-v` to the program.
- **Cache key.** The entry file and everything it imports, the compiler version, the target, which C compiler builds it, and the build flags (including `#if` symbols set on the command line). Upgrading Zinkite therefore rebuilds everything, and a binary from an older compiler is never run. File modification times can be checked first to skip rehashing unchanged sources.
- **Cache location.** `$XDG_CACHE_HOME/zink/`, or `~/.cache/zink/` if that is unset. The directory is created with mode 0700 and must be owned by the user, so another user can't plant a binary that `zink run` would execute.
- **Atomic writes.** Writing to a temporary file and renaming it means two runs at once never see a half-written binary; if both compile, the second rename harmlessly replaces the first.
- **Cost.** The first run pays for compilation: about a quarter of a second for a small script, most of it the C compiler (150–250 ms with clang or gcc, measured on 2026-11-20). Compiling the runtime once and caching it is the first thing to try if that needs to be faster (§17). Later runs cost a hash of the sources plus about 1 ms of startup.
- Compiling in memory and jumping into it, and running on a VM by default, were considered and rejected (`DESIGN-refcount.md`, Appendix A.19).

---

## 15. Performance

Measured on 2026-11-17 with hand-written C models of both designs (`bench/records.c`, `bench/gap_buffer.c`): Xeon W-2195 with a 25 MB L3, `cc -O2`, one run each. Every layout produced identical results.

**Records**: parse "id,name,qty,cents" lines, scan, sort by name, scan again, free. Nanoseconds per record:

| Workload | Previous design (reference counting) | This design | Faster by |
|---|---|---|---|
| 10,000 records, one run | 208 | 181 | 1.2× |
| 1,000,000 records, one run | 668 | 337 | 2.0× |
| 1,000,000 records, long-running | 1052 | 325 | 3.2× |

- **Memory**: 34 bytes per record against 90, since there is no header and the name is stored inline. That accounts for most of the gap at large sizes.
- **Freeing** is one store; the previous design spent 168 ns per record freeing a million sorted records.
- **At 10,000 records, string comparisons in the sort dominate** both designs.
- **Walking variable-size records in order** is slower in small data that fits in cache: 3.5 against 1.8 ns per record. Each record's position depends on the previous one's length.

**Cursor arrays**: measured again on 2026-11-18 with hole reuse (§4.5), plus a chunked index for comparison (`bench/gap_buffer.c`). Random insert plus remove, 16-byte elements:

| Elements | Plain array (`memmove`) | Classic gap buffer | Cursor layout (§4.3) | Chunked index, best chunk size |
|---|---|---|---|---|
| 1,000 | 153 ns | 125 ns | 62 ns | 128 ns |
| 10,000 | 1.96 µs | 1.89 µs | 249 ns | 310 ns |
| 100,000 | 33 µs | 26 µs | 4.9 µs | 1.1 µs |
| 1,000,000 | 571 µs | 747 µs | 111 µs | 6.9 µs |

- **Hole reuse costs nothing measurable.** The data array stayed at exactly n slots after every test, where before reuse 1,000 live elements had grown to 401,000 slots. Random edits and typing were as fast as before or faster, since the data now stays in cache.
- **Typing in one place takes 2–4 ns per element** with the cursor layout, at any size.
- **Random reads** cost 2.2 ns against 0.4 ns for a plain array at 1,000 elements, and 8.7 against 3.4 ns at a million.
- **Scans** cost the same while storage order follows logical order, and up to 4× more after many scattered edits.
- **The chunked index** splits the index into chunks of about √n entries (a tiered vector, Goodrich and Kloss 1999), so any insert or removal costs O(√n) instead of moving the gap:
  - **random edits get 12–16× faster** at a million elements, and 4× faster at 100,000;
  - **it is slower below about 50,000 elements**;
  - **typing in one place gets much slower**: every insert passes an entry through every later chunk, so it costs 52 ns at 1,000 elements and 5.6 µs at a million, against 2–4 ns with one gap;
  - **random reads cost about a third more** at a million elements (11.5 against 8.7 ns), because of the extra chunk lookup.

  So it doesn't replace the gap for cursor arrays, whose main use is editing in one place. It is available as a choice instead: a `$` after an array's slots selects it (§4.1).

**Hash maps**: measured on 2026-11-19 (`bench/hashmap_layouts.c`, median of 5 runs). Both layouts live in one region on their own reservation, store a 32-bit hash per entry, and run the same keys in the same order. The direct table is Goose's: one array of slots, doubled and rehashed in place. The compact one is §4.14's entries then index. Nanoseconds per operation, `u64` keys and `i64` values:

| Keys | Layout | Insert | Of which growing | Hit | Miss | Update | Iterate | Memory per key |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| 1,000 | direct | 17.5 | 6.1 | 2.7 | 4.4 | 3.3 | 1.63 | 49 B |
| | compact | 5.7 | 2.1 | 2.7 | 3.0 | 3.4 | 0.30 | 37 B |
| 100,000 | direct | 43.2 | 25.4 | 10.7 | 13.3 | 12.0 | 7.55 | 63 B |
| | compact | 21.6 | 7.6 | 15.5 | 11.3 | 16.4 | 0.86 | 37 B |
| 1,000,000 | direct | 49.9 | 23.0 | 29.9 | 29.9 | 95.2 | 7.61 | 50 B |
| | compact | 31.4 | 9.5 | 46.7 | 23.9 | 146.5 | 1.37 | 32 B |
| 10,000,000 | direct | 67.5 | 19.8 | 54.1 | 57.6 | 205.5 | 5.87 | 40 B |
| | compact | 67.8 | 15.3 | 94.8 | 85.5 | 303.2 | 1.66 | 31 B |

- **The direct table finds keys faster once the map outgrows the cache**, 1.3–1.75× from about 100,000 keys up, since a hit is one memory access instead of two. Below that, the two are equal.
- **The compact layout builds 1.5–3× faster up to a million keys**, grows 1.3–3× faster because only its index is rebuilt, iterates 3.5–6× faster, and uses 25–48% less memory, since an empty index slot is 4 bytes instead of a whole entry. Misses are usually faster too, except at 10 million keys.
- **String keys (8–24 bytes) follow the same pattern.** At a million keys: hits 141 against 209 ns for the direct table, inserts 71 against 107 ns and misses 76 against 104 ns for the compact one, and 64 against 39 MB.
- "Update" is `getOrAdd` of a key that is already there, then an increment. Removals weren't measured.

**Not measured yet**: long-running programs that keep adding and removing data, where memory only comes back to the data stack at scope exit (§3.4).

---

## 16. Prior art
- **Goose**: the source of the memory model:
  - data stacks freed at scope exit;
  - references that survive growth;
  - inline nested data;
  - roots inferred without annotations;
  - building in place;
  - relative references;
  - root structs, which Zinkite calls roots (§6.6);
  - compiling to C (§12.5), which Goose runs from source through a built-in TinyCC (§17).
- **Hylo** (formerly Val): mutable value semantics with second-class references.
- **Rust**: ownership and borrowing; `Vec`, `VecDeque` and slices; `saturating_add`; graphs through indices.
- **Zig** and **Go**: `defer`. **Zig** also has `+%` and `+|` for wrapping and saturating arithmetic; `i32`/`u8` type names and arbitrary-width integers; implicit integer conversions only where no value is lost; error unions as a light error channel.
- **Go**: interface values as a pointer plus a method table; `v, ok :=` results, the closest relative of the status code; character literals as untyped integer constants (`rune`); `cmp.Ordered` as a bound for built-in types.
- **C#**: `a[^1]` for indexing from the end.
- **Swift**: value types, mutating methods (Zinkite's `mut`), checked arithmetic; integer conversions that trap, with `truncatingIfNeeded:` and `clamping:` forms; `Comparable` and `Hashable`.
- **Java**: the syntax; compact source files for script files.
- **Gap buffers** (Emacs and other editors): the cursor layout's index array is a gap buffer.
- **Fredman & Saks**, "The Cell Probe Complexity of Dynamic Data Structures" (STOC 1989): the lower bound for indexing with inserts (§4.4).
- Prior art for the previous design (Swift, Nim, Lobster, Vale, CPython, Bacon & Rajan) is listed in `DESIGN-refcount.md`.

---

## 17. Open decisions

**Until a language feature listed here is decided, the compiler rejects programs that use it** (Decided). The error says the feature is undecided and names it. The rule covers the language items in this section and the syntax in `GRAMMAR.md` §8. The questions in `STDLIB.md` §11 are about how the library is designed, and gate nothing. Two items the checker meets in ordinary code: removing from an array inside a for-each by value over it, and `${x}` on a class value. Deciding an item later only makes more programs valid, so no program that compiles today can break.

**Memory and references**
- **Reference rules**: the open questions in `REFERENCES.md` §12, such as following roots per program point and choosing data stacks at run time.
- **Removing elements inside a for-each by value** over the same array (§9.4).
- **Enforcing `#max_memory_mb`** when growth never checks a capacity (§3.1, §8.2): commit memory in chunks from the page-fault handler and stop at the ceiling, which costs nothing on the fast path, or size each stack's reservation.
- **Threads**: Goose compiles each worker as a separate program with its own memory and typed queues between them.

**Arrays**
- **Hash map literals** with entries (`["a": 1]`), and `m[key]` indexing (`GRAMMAR.md` §8).
- **A second map type with Goose's direct layout**, for large maps that are mostly read. In maps too big for the CPU cache it finds keys 1.3–1.75× faster than `hashmap`, but iterates in no fixed order, and its references last only until the next insert (§15).
- **Making a window** (`T[<.<]`): the type is in §4.2, but nothing creates one yet.
- `*` on unbounded editable arrays (`T[..>*]`) (§4.6).
- Slices of arrays on data stacks whose elements aren't stored in order: cursor, chunked (`$`) and reversed (`>..`) arrays. Logical order is the natural choice for what a slice shows (§4.3, §4.10). Capped ring and gap buffers are decided: they become one block first (§4.10).
- Multi-dimensional arrays (the previous design had `array<int, 2>` with `m[i, j]`).
- Compile-time constants as type arguments, for caps (§10).

**Classes**
- **Tuples as values**: tuple variables, fields and array elements, and `t.0` (§9.5).
- Default and static methods in interfaces; variants stored inline (§6.3).
- Enums with fields and methods; pattern matching in `switch`.
- **Ordering by fields**: whether a class that says `implements ordered` without writing `compareTo` gets field-by-field order, like Rust's `derive(PartialOrd)` (§10.1).

**Errors**
- A stricter form of `x @failed = f();` that scopes `x` to the success branch (§7.5).
- Whether to add the `flag` mark-and-continue statement later (Appendix A.8).

**Other**
- Default formatting of class values in `${...}` (§4.13).
- Integers narrower than a byte (`i4`, `u4`): if wanted, Zig's approach, with any width from 1 to 64 bits that is packed only inside packed classes and arrays, which then can't hand out references (§9.1).
- Stored closures that capture (§9.3); method references.
- Static imports and aliases for whole packages (§8.5); the library search path (§8.4).
- **A direct machine-code backend**, only if depending on a C compiler becomes a problem (§12.5).
- **A built-in TinyCC for `zink run`** (maybe later; Decided until 2026-11-20, §12.5). It would cut the first run after an edit from about a quarter of a second to tens of milliseconds, and let `zink run` work with no C compiler installed. Against it: it has no `__int128` for `i128` and `u128` (§9.1), and its last release (0.9.27) lacks the `__builtin_add_overflow` family that every overflow check uses, so both would need slower fallbacks; it doesn't optimize; it's LGPL-2.1, so `zink` would link it as a shared library to keep its own code under Apache-2.0; and generated programs would be tested against another compiler. Caching a compiled runtime, so a first run compiles only the program's own code, is cheaper to try first.
- **Embedding model** (§13): TinyCC inside the host, precompiled `.so` plugins, a bytecode VM, or none; the host C ABI's shape.
- Smaller syntax items not in the grammar yet (map literals, `?.`, `assert`, `#elif` and others): `GRAMMAR.md` §8.
- **Standard library**: the open questions in `STDLIB.md` §11.

**Graphics** (needed by the `vendor.*` packages, `STDLIB.md` §10.6)
- **Calling C** is decided (§12.6). Still open: passing function pointers (callbacks) and structs by value, C strings C returns, and C's global variables.
- **C layout for classes**: a way to fix a class's field order, alignment and padding to C's, and to GLSL's `std140` and `std430` rules for uniform and storage buffers, so a slice of it can be uploaded as it is.
- **Vector and matrix types with arithmetic**, since there is no operator overloading: built-in types such as `f32x4`, or GLSL-like `vec3` and `mat4`, as Odin and Zig have. Otherwise `pos.add(vel.mul(dt))` is the only spelling.
- **Embedding a file at compile time**, such as a SPIR-V shader, into the binary.
- **Where `vendor.*` packages come from**: shipped with the compiler, or fetched and built on first use, as Goose's blessed packages are. This ties in with the library search path (§8.4).

---

## 18. Suggested tests

**Data stacks**
1. A growable local is freed at scope exit by restoring one stack top: a loop that fills a 1 MB `u8[..>]` each iteration never uses more than about 1 MB.
2. A reference returned by `push` on a `T[..>]` still refers to the same element after a million more pushes.
3. Two growable arrays live at once get different data stacks; growing them in alternation never moves either one.
4. `items.push(parse(line))` builds the record directly in the array, and returning a growable array by value doesn't copy it (IR-level tests).
5. Writing past a data stack's reservation hits its guard page and stops the program with a message.
6. A class with a `T[.*]` or `T[.]` field is variable-size: for-each over an array of it works, and `a[i]` is a compile error.

**Array types**
7. Every type in the §4.2 table parses with the layout in §4.3; `T[k]` is a compile error that suggests `T[.k]`.
8. `T[.~.$]` and `u32[~$]` behave exactly like `T[.~.]` and `u32[~]` under any sequence of edits (property test); `T[..>$]`, `T[.$]` and `u8[..~16$]` are compile errors; a function taking `T[.~.]&` accepts a `T[.~.$]`.
9. Methods the slots don't allow are compile errors: `pop` on `T[..>]`, `pushFront` on `T[..~]`, `insert` when the cursor slot is `.`, any editing method on a frozen array. Each operator needs the same capability as its method.
10. `*` with a numeric cap is a compile error; `T[>.>]` as a class field is a compile error; capped arrays nest inline in classes.
11. A function with a `T[..>]&` parameter accepts a `T[..~]` and a `T[>~>]`, is compiled once per layout, and can't call `pop` on the parameter.
12. `push` on a full capped array fails with `status.FULL`, leaves the array unchanged, and returns a reference to the last element; a cap of 0 is a compile error; `pop` on an empty array fails with `status.EMPTY` and returns the zero value.

**Operators**
13. `a +> 1 +> 2 +> 3` appends in order; `a ->` returns the last element and `a -<` the first; `+^`, `-^` and `^-` match `insert`, `removeBefore` and `removeAfter`.
14. `@failed = a +> 1 +> 2;` on an array with room for one element is `status.FULL`: the first failure is kept.
15. `a^-b` is a compile error whose message suggests `a ^ -b`; `a[^1]` and `a +^ x` in the same expression parse correctly.

**Cursor arrays**
16. `seek(i)` accepts 0 through `len`; `seek(len + 1)` fails with `status.POSITION` and leaves the cursor where it was; `seekFromEnd(0)` equals `len`; `move(n)` clamps at the edges.
17. Any sequence of inserts, removals and seeks leaves the same logical order as a plain array that does the same edits (property test).
18. After removals, inserts reuse the holes: the data array never holds more slots than the most elements live at once; removing an element while a reference into the array may still be used is a compile error.

**Indexing and strings**
19. `a[^1]` is the last element; `a[^0]` and `a[^k]` with `k > len` stop the program; `^k` outside brackets is a compile error; `a[2:^1]` slices.
20. `a[-1]` stops the program; `a[%-1]` is the last element and `a[%len]` the first; `a[|-5]` is the first and `a[|len + 5]` the last; all of them are out of range on an empty array.
21. `a.getOr(i, fallback)` returns the fallback when `i` is out of range.
22. String literals are views of static data; `==` compares contents; interpolation builds into its destination; `split` returns views into its input with no copying.
23. `println("${a} ${b}")` and `out.append("${x.hex()}")` build no temporary (IR-level test); `u8[.] msg = "${n} files";` keeps its text for `msg`'s whole scope; `log("${n} done")` uses a temporary on the scratch data stack that is released at the end of the statement; `string s = "${x}";` is a compile error that suggests `u8[.]`.
24. `s[i]` is a `u8`; `for (u32 c : s.codePoints())` decodes UTF-8, including 4-byte sequences.
25. `line[i] == ','` compiles with no cast; `u8 b = 'é';` and `u16 c = '\u{1F600}';` are compile errors; `var c = 'a';` is a `u32` holding 97; `char` is a compile error that suggests `u32`.
26. `${n}` prints a number in decimal, `${s}` a string as text, and `${a}` an integer array as a list (`[104, 105]`); `${a.utf8()}` prints the same array as `hi`.
27. `hex()`, `bin()`, `pad(w)`, `zeroPad(w)` and `fixed(d)` format as described in §4.13, inside any interpolation (print, a string, a file write); `c.utf8()` on a surrogate or a value above 0x10FFFF writes `�` with `status.INVALID`.
28. `errprintln` writes to stderr and `println` to stdout: with stdout redirected to a file, the error messages still reach the terminal; `stderr.println(x)` and `errprintln(x)` do the same.

**References** (more in `REFERENCES.md` §10)
29. `Item x = items[0]; x.heavier(1.0);` leaves `items[0]` unchanged; with `Item& r = items[0];`, `r.heavier(1.0)` changes it, `r = v` writes through, and `r .= items[1]` rebinds.
30. `inc(count)` with `void inc(i64& n)` changes `count`; `inc(&count)` does the same with a warning; a plain parameter given `count` can't change it.
31. The `longest` example (§5.2) is a compile error that names `inner` as the root.
32. Returning a reference to a function's own local is a compile error; returning one rooted at a parameter compiles, and at the call site it takes the argument's root.
33. A reference into a variable-size element that is live across a removal is a compile error.
34. Using a `T&?` without a null check the compiler can see is a compile error; inside `if (r != null)` it compiles.

**Classes and interfaces**
35. A class's size is exactly its fields plus alignment, with fixed-size fields at fixed offsets ahead of variable-size ones (layout dump).
36. A method without `mut` that writes a field, directly or through a call, is a compile error; so is calling a `mut` method on a plain parameter.
37. Reading a field before the constructor assigns it, or ending a constructor with a field unassigned, is a compile error.
38. A class with no constructor can be built with `new C(...)` taking its fields without initializers, in declaration order; fields with initializers and editable arrays aren't parameters; a class that declares a constructor has no default one.
39. `class A extends B` and `protected` are compile errors.
40. An interface reference is 16 bytes and calls through the method table; `Shape s;`, a `Shape` field and `Shape[..>]` are compile errors.
41. `if (s instanceof Circle c)` binds `c` only when `s` refers to a `Circle`.
42. A generic function with `<T implements Shape>` is compiled once per type, with direct calls (IR-level test).
43. `new` with a non-class type is a compile error; `items.push(new(1, 0.0))` infers the class.

**Cleanup and resources**
44. `defer` statements run in reverse order when their block ends, including through `return`, `break` and `continue`, and don't run when the program stops; a deferred `return`, `break`, `continue` or status write is a compile error.
45. Declaring `delete()` is a compile error that suggests `defer`; any class value can be copied, and freeing an array of them does no per-element work.
46. Closing a `File` through one copy makes operations through another copy fail with `status.BAD_HANDLE`; `close()` fails with `status.IO` when buffered data can't be written; a file left open is flushed and closed at exit and when the program stops, with failures reported on stderr; the lint warns about a local `File` that may be left open.

**The status code**
47. A function that never sets a status returns 0 and carries no hidden channel (IR-level test).
48. `return #1 x;`, `return #status.FULL x;`, `return #(cond) x;`, `return #(2) x;` and `return #(@(g())) x;` work; `#300`, `#(count)` with an `i64` `count`, and a bare `return @(g())` are compile errors.
49. `x @failed = f();` gets both, `@failed = f();` the status only, and `x = f();` the value only, with a warning if `f` can fail.
50. `@a.pop()` parses as `@(a.pop())`; `@a + b` is a compile error that asks for parentheses.
51. Every standard-library failure returns the zero value with the named code it documents; `status.message(status.NOT_FOUND)` is "No such file or directory"; `status.NOT_FOUND == status.ENOENT == 2`; `status.EMPTY` is 192; an `EPERM` from the operating system arrives as `status.PERMISSION`; `status.message(230)` is "Error 230".
52. `@a.pop() || @b.pop()` is true when either fails; in `@f(g())`, `g`'s dropped status warns; the status of a chain is its first failure.
53. `num @_ = f64.parse(key);` and `@_ = f();` don't warn; `_ = f();` discards a value of any type; `(i64 a, i64 _) => a` and `for (var _ : items)` compile; `if (_)`, `x = _` and `_ += 1` are compile errors; `_count` is an ordinary name.
54. `#if`, `#max_memory_mb` and `return #1` lex correctly in the same file.
55. `if (err)` with a `u8` `err`, `while (n)` with a `u32` `n`, and `!err` compile; `if (n)` with an `i64` `n`, `bool b = err;` and `if (x = 0)` are compile errors.

**Bugs stop the program**
56. Overflow, `x / 0`, `MIN / -1`, an out-of-range index, stack overflow, and exceeding `#max_memory_mb` each stop the program with a message naming the source line and exit status 1, after flushing open files.
57. At the limits of every integer type, signed and unsigned, `+` stops the program, `+%`, `-%` and `*%` wrap, and `+|`, `-|` and `*|` saturate; `MIN % -1` is 0.
58. `long`, `byte` and `double` are compile errors that name the new type, and `int` and `float` used as types are errors that say they are bounds and suggest `i64` and `f64`; `u8 b = 300;` is a compile error; `var x = 5;` is an `i64`; `i64 big = 100000 * 100000;` doesn't overflow; `i64 n = someI32;` compiles without a cast, and `i32 m = someI64;` needs one.
59. A program printing many lines, run as `zink run tool.zn | head -1`, exits quietly with no error output.

**Directives**
60. An unknown directive, a directive outside the main file, and a duplicate directive are compile errors.
61. `#if` around directives selects per-build settings; precedence is host > build flag > directive > default.
62. Both `#if` branches must parse; only the active one is type-checked.

**Command-line tool**
63. `zink run` passes the program's exit code, stdin/stdout and signals through unchanged, and arguments after the source file reach the program untouched.
64. A second `zink run` of unchanged sources doesn't recompile; changing any imported file, the compiler version or a build flag does.
65. Two concurrent `zink run`s of the same new script both succeed and never execute a half-written binary.
66. `zink run` refuses to use a cache directory that is writable by other users or owned by someone else.
67. `zink run` (`-O0`) and `zink build` (`-O2`), each with clang and with gcc, produce the same output on the whole test suite.

**Hash maps**
68. For-each over a `hashmap` visits entries in insertion order, and the output is the same across runs although the hash seed differs.
69. `get` on a missing key returns the zero value with `status.MISSING`; `remove` of a missing key fails with `status.MISSING`; `put` replaces an existing value.
70. A reference from `getOrAdd` still refers to the same value after a million further inserts; using it after a `remove` or `clear` of that map is a compile error.
71. After removing most entries, the map compacts: memory stays bounded and insertion order is kept.
72. `hashmap<f64, i32>`, a variable-size key or value type, and a `hashmap` whose values are growable arrays or maps are compile errors; a class key with all-hashable fields works.
73. A `hashmap<string, i64>` whose keys view a buffer declared after the map is a compile error (rule S).

**Packages, imports and scripts**
74. `import a.b.C as D;` makes `D` mean `a.b.C` in that file only, and leaves `C` unbound there.
75. Two imports of the same simple name are a compile error that suggests `as`; an explicit import beats a wildcard; a name from two wildcards is an error only where it's used.
76. A `package` line that doesn't match the file's directory is an error, and the source root is inferred correctly from the entry file.
77. Importing a package-private declaration from another package is an error; circular imports between packages compile.
78. A file with only top-level methods and fields compiles; `main()` and `main(string[.] args)` both work, returning `void` or `u8`; importing from a script file is a compile error.
79. `u8 main` returning 3 exits with status 3, and falling off its end exits with 0; `return #1;` in `main` and `return 300;` in `u8 main` are compile errors; `exit(2)` from a helper exits with 2 without running `defer`s, after flushing open files.
80. Static initializers run before `main` in dependency order; initializers that depend on each other in a loop are a compile error that lists the loop.

**Lambdas and statements**
81. `filter((Dog d) => d.age >= minAge)` compiles to the same code as the handwritten loop (IR-level test).
82. Storing a capturing lambda in a `func` variable or field is a compile error; a non-capturing one works.
83. Assigning a captured variable, inside the lambda or outside it, is a compile error.
84. `var` copies; `var` without an initializer, `var x = null;` and `var` on a field are compile errors.
85. Appending to an array inside its own for-each never reads moved memory, and the loop sees the new elements.
86. A `switch` expression that misses an enum constant and has no `default` is a compile error; cases never fall through.
87. `for (i64 x : new Countdown(3))` prints 3, 2, 1; `break`, `continue` and `return` inside a loop over an iterator behave as in an array loop; a class with an `iterator()` method can be the target of a for-each.
88. A for-each over `s.codePoints()` or `s.lines()` builds no array and compiles to the same code as the hand-written loop (IR-level test); removing from the text inside the loop is a compile error.
89. `for (var i = 0; i < 100; i++)` makes `i` an `i64`.
90. `(string cmd, string arg) = splitFirst(line);` declares both; `(cmd, arg) = ...` assigns existing variables; `(string w, _) = ...` discards one; `(i64 q, i64 r) @err = divMod(a, b);` also captures the status; `(x, y) = (y, x);` swaps.
91. `(string, string) t = f();`, `f().0`, a tuple field, `(string a, b) = f();` (mixing declared and existing targets), and using a tuple result inside a larger expression are compile errors.
92. Two `string` results come back in registers, with no store to memory (IR-level test); storing a result of `splitFirst(inner)` where it outlives `inner` is a compile error naming that result's roots.

**Casts, mixed types and bounds**
93. With `n` = 300: `(u8) n` stops the program, `(u8%) n` is 44, `(u8|) n` is 255, and `u8.from(n)` is 0 with `status.RANGE`; `(u8) 300` is a compile error; `(u8) (x & 0xFF)` compiles with no check (IR-level test).
94. `(i64) 3.7` is 3 and `(i64) -3.7` is -3; a cast of NaN, or of `1e10` to `i32`, stops; `(u8|)` of NaN is 0; `(u8%) 2.5` and `(f32%) x` are compile errors; `(f64) i64.MAX` is 2⁶³, and `(f32)` of `1e300` is infinity, without stopping.
95. `n + b` with an `i32` and a `u8` is an `i32`; `c + d` with a `u32` and an `i32`, and `h % len` with a `u64` and an `i64`, are compile errors that suggest a cast; `x += len` with an `i32` `x` is a compile error.
96. `d < c` is true for `i32 d = -1` and `u32 c = 0`; comparisons of `u64` with `i64` are right at both types' limits; `a[h]` with a `u64` `h` above `i64.MAX` is out of range.
97. `sum` with `<T implements number>` works for every number type; `%` in its body is a compile error; `T total = 0;` compiles and `T k = 1000;` doesn't; `(T) 1000` is a compile error only in the `i8` and `u8` instances.
98. `ordered x;`, `number& r` and `int[..>] a` are compile errors; a declaration named `ordered` or `uint` is a compile error; a class can't implement `number`.
99. A class that implements `ordered` with `compareTo` supports `<`, `<=`, `>` and `>=` in plain and generic code, and without `compareTo` it is a compile error (until §17 decides on ordering by fields); `"apple" < "banana"`, `false < true`, and enum constants in declaration order hold.
100. `sort` and `max` over `f64` values that include NaN put NaN last and order the other values correctly; `hashset<f64>` and a `hashmap` keyed by a class with an `f64` field are compile errors.

**Repeat loops**
101. `while (Pending p : todo ->)` pops every element once, in stack order, and doesn't run the body after the failing pop; `continue` makes the next call, and `break` makes none.
102. `while (DirEntry e : d.next(names))`, a `T&` binding from a `mut T& next()`, and a destructured tuple result compile; `while (var it : s.lines())`, and a source that isn't a call or an array operator, are compile errors.
103. `while (string line : stdin.nextLine(buf))` compiles although `nextLine` clears `buf`; storing `line` in an array declared before the loop is a compile error, since `buf` is removable and a view into it can't be stored in an array (`REFERENCES.md` §5.3).
104. `for (string l : lines)` over a `LineIterator` variable is a compile error that suggests `while (string l : lines.next())`; `for (string l : text.lines())` compiles.
105. After `while (JsonEvent e : r.next()) @why { ... }`, `why` is `status.END` for a complete input, the error's code for a malformed one, and 0 after a `break`; `while (a ? b : c)` parses as a condition.

**Overloading and lambdas**
106. With `write(string)`, `write(i64)`, `write(u64)` and `write(f64)`: `write(42)` calls `write(i64)`, a `u32` argument calls `write(u64)`, an `i32` argument calls `write(i64)`, and an `f32` argument calls `write(f64)`.
107. With only `f(i32)` and `f(u8)`, `f(1)` is an ambiguous call, a compile error that lists both; `f(Item x)` beside `f(Item& x)`, and two overloads that differ only in their return types, are compile errors.
108. A `Shape&` argument that refers to a `Circle` calls `f(Shape&)`, not `f(Circle&)`; a non-generic overload beats an equally good generic one; a generic body calling `write(x)` with `x` of a `number` type parameter is a compile error.
109. `(a, b) => a + b`, `d => d.age` and `(var a) => a` are compile errors; `(i32 a, i32 b) => { return a + b; }` and `(Dog d) => d.age >= 18` compile; a lambda whose parameter types differ from its `func` parameter's is a compile error.
110. A class with `new(f64 w, f64 h)` and `new(f64 side)` can be built with either.

**Definite assignment**
111. A constructor that assigns a field only inside an `if` without an `else`, only inside a `while (n > 0)` loop, or only in some arms of a `switch` is a compile error naming the field; one that assigns it in both branches of an `if`, inside a `do` loop, or before every `break` of a `while (true)` compiles.
112. A constructor that calls a method on `this`, or passes `this`, before its last field assignment is a compile error; after it, both compile.
113. Reading a local declared without an initializer, where some path hasn't assigned it, is a compile error.
114. `new(f64 side) { this.new(side, side); }` builds the same value as `new(side, side)`; assigning a field or using `this` before `this.new`, and a `this.new` that can run twice, are compile errors.
115. A constructor that returns `#status.INVALID` before assigning any field gives the zero value with that status; in a class with a `T&` field, returning early without assigning it is a compile error.

**Slices and writable views**
116. `a[2:5]`, `a[:3]`, `a[2:]`, `a[:]` and `a[1:^1]` give the expected views; `a[c ? i : j]` is an index; `a[5:2]` and `a[0:a.len + 1]` stop the program.
117. `rows[1:].sort()` sorts all but the first element; `buf[0:4] = "abcd"` copies four bytes, and a three-byte right side stops the program; a copy between overlapping parts of one array is correct.
118. A writable view of a string literal, of a `T[:]` or of a plain parameter is a compile error; `.=` rebinds a `u8[:]&` to another range; removing from an array while a writable view of it is live is a compile error.
119. `.=` on a value variable is a compile error that suggests `=`; `var& c = counts.getOrAdd(w, 0); c++;` changes the map's value.
120. `println(out)`, `line.trim()`, `new JsonParser(text)` and `cmd == "quit"` on `u8` arrays compile and view them; `var c = out;` copies; `string s = buf;` followed by `buf.clear()` while `s` is still used is a compile error; passing a cursor array where a `string` is expected is a compile error; with `f(string)` and `f(u8[..>]&)`, `f(buf)` calls the second.
121. `hashmap<string, u8[..>16]>` and `Pair<string, Pair<i32, i32>>` parse: a `>` inside slot brackets belongs to the slots, and `>>` closes two type-argument lists.
122. A bare `println(...)` draws no dropped-status warning; when its write fails, the program reports it on stderr at exit and exits with status 1.
123. `clear()` through a `T[..>]&` parameter is a compile error; through a `T[..~]&` it compiles.
124. A program's own `sum` or `splitFirst` hides the `zink.lang` one, with no error; two imports of the same name are still an error.
125. With `string w = a;`, `w = b;` makes `w` view `b`; with `u8[:]& v = buf[0:3];`, `v = "abc";` copies three bytes into `buf`.

**Roots**
126. A `root` with two growable fields and a cursor-array field compiles, and each part grows independently of the others.
127. Copying a root, passing it as a plain parameter, returning it by value, storing it in a class, a root or an array, and a `new Parser()` that isn't assigned to a named variable are compile errors; `Parser&` parameters and fields work.
128. A second growable field, or a cursor-array field, in a `class` is a compile error that suggests `root`.
129. `Node& root = tree.push(...)` and a field named `root` compile.

**Second example batch** (`examples/NOTES.md` §5)
130. In a root holding a pool and a map, removing from the map while references into the pool are live compiles; removing from the map while a reference into the map is live is a compile error.
131. After `if (r == null) { return; }`, `r` is narrowed for the rest of the block; a field compared with `null` isn't narrowed, and rebinding a narrowed local with `.=` ends its narrowing.
132. In a constructor, `this.next = n;` binds an unassigned reference field, and a second `=` writes through it; a class with a `hashmap` field and no constructor is built with `new C()`, the map starting empty.
133. `return #why;` with a `u8` variable `why`, and `return #done;` with a `bool`, compile.
134. With `string path` in scope, `path.base(x)` calls the variable's method; outside that scope it names the `path` class.
135. `summarize(ints)` with an `i64[..>]` instantiates `T = i64`; `max(a, b)` with a `u8` and a `u32` is a compile error.
136. Using a feature still open in §17 is a compile error that names it; calling `stableSort` or importing `zink.json` isn't.

---

## Appendix A. Superseded and rejected designs

### A.1 Reference counting with an acyclic heap (the previous design)
Objects on a heap, each with a 16-byte header and a 64-bit count, freed on the line their last strong reference went away. The design had several parts:
- a compile-time rule kept the heap acyclic: no strong field could lie on a cycle in the class graph;
- `owned` fields, `owner` back-pointers and `weak` references covered structures with back links;
- `borrow` and `fresh` modes and `take`/`replace` moved ownership;
- destructors ran in two phases.

It is kept in full, with its own Appendix A of alternatives (trial deletion, the referrer table A.2, the hybrid A.24 and others), in `DESIGN-refcount.md`.

Superseded on 2026-11-17. The benchmarks in §15 showed it 1.2–3.2× slower on a records workload, mostly from per-object memory and freeing, and the syntax (`owned`, `owner`, `weak`, `borrow`, `fresh`) was felt to be too complex. What was given up:
- objects that point at each other freely;
- inheritance;
- exceptions;
- memory that comes back as soon as it is unused, in long-running programs.

### A.2 Exceptions
The previous design used Java's `try`/`catch`/`finally`/`throw`, with `Error` for bugs and `Exception` for expected failures (`DESIGN-refcount.md` §12.7). Replaced by the status code (§7) and by stopping the program on bugs (§7.6), so nothing is thrown and there is no unwinding. What was lost: an exception could carry a message and details with the error. A status carries only the reason (§7.7), and the caller writes the message.

### A.3 Goose's multi-frame return
`return err from load` returns from several frames at once, checked statically, with no unwinder and no `Result` type. Considered as the error mechanism and not chosen: a status per call was preferred.

### A.4 Named array types
`array<T>`, `list<T>`, `deque<T>`, `gaplist<T>` and `inline list<T, 16>`, mapped onto the same layouts, with methods only. Rejected in favor of the slot grammar (§4.1). The slot grammar states the capabilities in the type itself, and its operators were wanted.

### A.5 References by default for class types
Java's behavior: `Item x = items[0];` would bind a reference, with copying written explicitly. Rejected for Goose's rule that `=` copies (§5.1). The memory model depends on references being visible, and copies avoid accidental aliasing.

### A.6 Single inheritance
Java's `extends`. Rejected: subclass values are larger than their base, so they can't be stored inline in an array of the base type, and virtual calls would need a class id in every value. Interfaces with 16-byte references (§6.3) provide dynamic dispatch without either.

### A.7 `T[k]` for fixed-size arrays
Replaced by `T[.k]`, so every array type is written with the slot grammar.

### A.8 A `flag` statement
"Mark the status now, return later". Dropped from the first version, since `return #(cond) x;` covers the cases seen so far. It can be added later without breaking anything.

### A.9 Tombstones and swap-remove for cursor arrays
Tombstones would make indexing O(n) or need a rank structure; swap-remove breaks order. The gap layout (§4.5) keeps O(1) indexing and order.

### A.10 A `char` type
Java's `char` is a 16-bit UTF-16 code unit, so characters outside the first 65,536 code points, such as emoji, take two of them. The previous design had a 4-byte `char` holding one code point, as Rust does. Replaced by plain `u32` code points (§9.1):
- text is UTF-8 bytes, so a byte is compared with a character constantly: `line[i] == ','` should work without a cast or a separate byte-literal syntax;
- a separate type would add little now that printing is chosen by method (§4.13).

What was given up: the type no longer guarantees that a value is a real code point, which is checked when it is turned into text instead.

### A.11 Format specs and print families
Two other ways to format values were considered:
- **Specs inside interpolation**, as in C# and Rust: `${n:x}`, `${x:.2}`. They are shorter, but need a small format language and a parsing rule for `?:` inside `${...}`, and a program can't add its own.
- **One print function per format**: `printn`, `printc`, `printx`, `printlnx` and so on. The names multiply with every new format and output (stderr, files, buffers), they work only for printing, and one line of output takes several calls.

Formatting methods (§4.13) were chosen instead.

### A.12 Destructors
A class could declare one `delete()`, run when a value's scope ended and once per element when an array of that class was freed (Proposed for this design; the previous design had two-phase destructors). Dropped to match Goose:
- freeing stays one store with no per-element work;
- every class can be copied, where a class with a destructor couldn't have been;
- there's no zeroed slot left behind for stale references to see.

What was given up: a forgotten `close()` is no longer closed when its scope ends, only at exit (§6.4). `defer` and a lint cover most of that.

### A.13 Separator dots in slot types
The pasted notes wrote `T[start.middle.end cap]`, with dots between the slots and an empty slot written as nothing: `T[>.~.>]`, `T[..>]`, `T[..]`. Replaced by one character per slot, with `.` meaning "nothing", plus a one-character shorthand (§4.1):
- positions are fixed, so a type reads left to right as start, cursor, end;
- a dot always means the same thing, where before it was sometimes a separator and sometimes part of an empty slot;
- types and slices no longer look alike: `T[..n]` was a capped type and `a[..n]` a slice.

Most common types are spelled the same either way (`T[..>]`, `T[..~]`, `T[.~.]`, `u8[..~16]`). The ones that changed include `T[>.>]` (was `T[>..>]`), `T[>~>]` (was `T[>.~.>]`), `T[~]` (was `T[~.~.~]`) and the frozen `T[.]` (was `T[..]`).

### A.14 A one-bit status
The pasted notes gave every call a one-bit side output, 0 or 1, read as a `bool`. Replaced on 2026-11-18 by a `u8` status (§7), so a failed call can also say why: "no such file" or "permission denied", which CLI tools need for their messages. The value `u8 main` returns becomes the exit status. To keep `if (failed)` working, unsigned integers became usable as conditions (§9.4). A separate `status` type with that one property was considered and found unnecessary.

