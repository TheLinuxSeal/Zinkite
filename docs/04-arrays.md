# 4. Arrays

An array's type says what can change about it: whether elements can be added or removed
at its start, at its end, or at a cursor in the middle, and where its elements live. The
compiler picks a layout from that, and allows only the operations the type names. This
chapter covers the slot grammar and every kind of array it spells, array literals, the
operators and their methods, indexing, slices and writable views, for-each, and the
methods that sort, search and build arrays. Text is a `u8` array, and has its own chapter,
[Text](03-text.md). Which references into an array survive which changes is in
[Memory and references](05-memory.md).

## The slot grammar

An array type is the element type followed by brackets holding three **slot characters**,
for the start, the cursor and the end, in that order:

```
T[ s c e  cap? ]        three slot characters: start, cursor, end
T[ a  cap? ]            one character, standing for all three: T[~] is T[~~~]
```

Each slot says what can change at that position:

| Symbol | Meaning |
|---|---|
| `.` | nothing can change here |
| `>` | insert |
| `<` | remove |
| `~` | insert and remove |

So `i64[..>]` grows at its end and nothing else, `i64[..~]` is a stack, `i64[~.~]` a deque,
`i64[.~.]` can insert and remove at a cursor, and `i64[.]`, which is `i64[...]`, can't
change length at all. One character stands for all three slots: `T[~]` can do everything.

After the slots comes an optional **cap**, which says where the elements live:

| Cap | Meaning |
|---|---|
| a number (`16`) | inline storage for that many elements, in the variable, field or element that holds the array |
| `*` | a variable-width (varint) length, for frozen arrays only |
| `$` | a chunked index, for cursor arrays only |
| *(none)* | on a data stack when the array can grow, or sized when it is built when it can't |

The slots say which operations are allowed; the cap says where the data lives
([DESIGN §4.1](../DESIGN.md#41-type-grammar-decided)).

```java
void main() {
    i64[..>] log = [];          // grows at its end, on a data stack
    i64[..~] stack = [];        // push and pop at the end
    i64[~.~] queue = [];        // both ends
    i64[~] edit = [];           // both ends and a cursor
    i64[.] frozen = [1, 2, 3];  // length fixed when it is built
    i64[.4] four = [];          // exactly 4 elements, inline
    u8[..~16] name = "ada";     // up to 16 bytes, inline
    log +> 1;
    stack +> 2;
    queue +< 3;
    edit +^ 4;
    println("${log} ${stack} ${queue} ${edit} ${frozen} ${four} ${name}");
}
```

```output
[1] [2] [3] [4] [1, 2, 3] [0, 0, 0, 0] ada
```

- **A type has one slot character or three**, before any cap. Two is an error:

```java
void main() {
    i64[~~] a = [];
}
```

```error
example.zn:2:8: error: an array type has one or three slot characters, not 2
```

- **`*` is for frozen arrays**, whose slots are all `.`, and `$` needs a cursor slot other
  than `.`:

```java
void main() {
    i64[..~*] a = [];
    i64[..~$] b = [];
}
```

```error
example.zn:2:5: error: `*` is allowed only on frozen arrays, whose slots are all `.` (DESIGN §4.6)
example.zn:3:5: error: `$` needs a cursor slot other than `.` (DESIGN §4.1)
```

- **There is no `T[k]`.** `T[.k]` is the only fixed-size spelling.
- **In a type, the characters between the brackets are read as slots**, so `>>` and `<<`
  count as two slot characters there, though they are shift operators elsewhere.
- **Arrays nest**: `i32[.2][.3]` is an array of 3 elements, each an `i32[.2]`.
- **A cap can be a constant**, as in `u8[..~MAX_NAME]` after `static final i64 MAX_NAME =
  16;`, or `Limits.MAX_NAME` from another class ([Constants](06-classes.md#constants)). A
  plain static can't be one.

## The kinds of array

Each kind below is a choice of slots and cap. The compiler picks the layout
([DESIGN §4.3](../DESIGN.md#43-layouts-the-compiler-picks-decided)); a program never names
one.

| Type | What can change | Where it lives and how |
|---|---|---|
| `T[.k]` | nothing: always exactly `k` elements | inline; no length is stored |
| `T[.]` | nothing: the length is set when it is built | sized at construction; in a record, an 8-byte length and then the elements |
| `T[.*]` | nothing, as `T[.]` | as `T[.]`, with a varint length (1 byte up to 127) |
| `T[..>]` | insert at the end | a data stack; a header `{p, n}` pointing at the elements |
| `T[..~]` | insert and remove at the end | the same |
| `T[..<]` | remove at the end | the same |
| `T[..>k]`, `T[..~k]` | as above, up to `k` elements | inline: a count as narrow as `k` allows, then the elements |
| `T[>..]`, `T[~..]` | insert (and remove) at the start | a data stack; a capped one sits at the top of its buffer |
| `T[>.>]`, `T[~.~]` | both ends | a data stack, starting in the middle of its reservation so either end can grow |
| `T[~.~k]` | both ends, up to `k` | inline ring buffer `{h, n, e[k]}` |
| `T[<.<]` | remove at both ends | a window that only narrows |
| `T[.~.]`, `T[>~>]`, `T[~]` | at a cursor (and at the ends) | two data stacks: elements that never move, and an index of 32-bit slots in two deques, before and after the cursor |
| `T[.~.k]`, `T[~k]` | at a cursor, up to `k` | inline gap buffer `{gs, an, cur, e[k]}` |
| `T[.~.$]`, `T[~$]` | at a cursor | as a cursor array, with the index in chunks of about √n entries |

What the operations cost ([DESIGN §4.4](../DESIGN.md#44-cost-model-decided)):

| Operation | Cost |
|---|---|
| push or pop at the end, pushFront or popFront at the start | O(1) |
| insert or remove at the cursor | O(1) |
| `seek(i)`, moving the cursor | O(distance), moving 4-byte indexes; free with `$` |
| insert at a random position (`insertAt`) | O(n) at worst; O(√n) with `$` |
| `a[i]` | O(1): one load, two with a cursor slot, three with `$` |
| `clear()` on a data stack | one store |

- **Use the plainest type that does the job.** A cursor slot makes every read take two
  loads, even in code that never moves the cursor, so an array that is mostly read should
  have `.` in the middle.
- **Each declaration of a growable local gets data stacks of its own**, and an array on a
  data stack is freed when its scope ends, with one store
  ([Memory and references](05-memory.md)).
- **Capped arrays nest inside classes and other arrays**, since they are inline. An array
  on a data stack can be a class's one growable field, and a cursor array, which takes two
  data stacks, needs a root ([Classes, interfaces and generics](06-classes.md)).

## Creating arrays

An array literal lists the elements in brackets. `[]` is an empty array. Neither has a type
of its own: it takes the type of the variable, parameter or field it initializes.

```java
class Point {
    i64 x;
    i64 y;
}

i64 total(i64[:] xs) {
    i64 t = 0;
    for (i64 x : xs) {
        t += x;
    }
    return t;
}

void main() {
    i64[..>] xs = [5, 2, 8];
    f64[..>] fs = [1, 2.5];
    Point[..>] ps = [new Point(1, 2), new Point(3, 4)];
    string[..>] words = ["one", "two"];
    i64 n = 3;
    i64[..>] computed = [n, n * 2, total([n, 1])];
    i32[.4] zeros = [];
    println("${xs} ${fs} ${ps[1].y} ${words} ${computed} ${zeros} ${total([1, 2, 3])}");
}
```

```output
[5, 2, 8] [1.0, 2.5] 4 [one, two] [3, 6, 4] [0, 0, 0, 0] 6
```

- **The literal's elements take the element type**, as any literal takes its type from
  where it's used: in `f64[..>] fs = [1, 2.5]` the `1` is an `f64`.
- **`[]` with nothing to go by is an error**:

```java
void main() {
    var x = [];
}
```

```error
example.zn:2:13: error: `[]` takes its type from where it's used, and nothing says it here
```

- **A fixed array's literal gives all `k` elements, or is `[]`** for `k` zero values. A
  capped array's literal can't be longer than its cap. Both are checked when compiling:

```java
void main() {
    i64[.3] x = [1, 2];
    i64[..~2] y = [1, 2, 3];
}
```

```error
example.zn:2:17: error: an array of type `i64[.3]` holds exactly 3 elements, and this literal has 2
example.zn:3:19: error: this literal has 3 elements, and the array holds 2
```

- **Assigning an array copies it.** `i64[..>] b = a;` and `var c = a;` each make a copy with
  elements of their own; `var` keeps the array's type. To refer to the elements instead,
  take a slice (below) or a reference ([Memory and references](05-memory.md)).
- **A slice converts to any kind of array**, copying its elements: `i64[.] f = xs[1:];`,
  `i64[..~4] c = xs[:];`. Converting to a fixed `T[.k]` needs exactly `k` elements, and
  to a capped array at most `k`, or the program stops.

```java
void main() {
    i64[..>] a = [1, 2, 3];
    i64[..>] b = a;
    var c = a;
    b +> 4;
    c[0] = 9;
    i64[.] f = a[1:];
    i64[..~4] capped = a[:];
    i64[.3] fixed = a[:];
    println("${a} ${b} ${c} ${f} ${capped} ${fixed}");
}
```

```output
[1, 2, 3] [1, 2, 3, 4] [9, 2, 3] [2, 3] [1, 2, 3] [1, 2, 3]
```

```java
void main() {
    i32[..>] c = [1, 2, 3];
    i32[.4] part = c[0:2];
    println("${part}");
}
```

```stderr exit=1
example.zn:3: the value isn't the 4 elements the array holds
```

- **Today a capped or fixed array is initialized from another array through a slice**, as
  above. `i64[.3] fixed = a;` without the `[:]` passes the checker, but the C backend
  generates code the C compiler rejects ([What isn't there yet](15-not-yet.md)). Between
  arrays on data stacks, `i64[..~] d = a;` works directly.

## Arrays that grow at the end: `T[..>]` and `T[..~]`

These are the everyday arrays. Each lives on a data stack of its own, grows without moving
anything, and is freed when its scope ends. `T[..>]` only grows; `T[..~]` can also shrink
at the end, which makes it a stack.

```java
class Item {
    i64 id;
    f64 weight;
}

void main() {
    Item[..>] items = [];
    Item& first = items.push(new Item(1, 0.5));
    for (i64 i = 2; i <= 100000; i++) {
        items +> new Item(i, 0.0);
    }
    first.weight += 1.0;
    println("${items.len} items, the first weighs ${items[0].weight}");

    i64[..~] stack = [];
    stack +> 1 +> 2 +> 3;
    while (i64 x : stack ->) {
        print("${x} ");
    }
    println("and the stack is empty: ${stack.len}");
}
```

```output
100000 items, the first weighs 1.5
3 2 1 and the stack is empty: 0
```

- **`push`, and `+>`, return a reference to the new element**, which stays valid as the
  array grows, since its elements never move ([DESIGN §3.1](../DESIGN.md#31-data-stacks-decided)).
- **`pop`, and `->`, return the removed value**, or the zero value with `status.EMPTY` when
  there is nothing to pop. The repeat loop `while (i64 x : stack ->)` pops until the stack
  is empty ([Errors and statuses](07-errors.md)).
- **A removal can't happen while a reference into the array may still be used**: popping
  while `first` above is still used later would be a compile error (rule R,
  [Memory and references](05-memory.md)).
- **An array keeps the slots it frees** for its own later pushes. Its memory goes back only
  when its scope ends.

## Frozen arrays: `T[.]` and `T[.*]`

A frozen array's length is chosen when it is built and never changes. Its elements can
still be written, swapped and sorted.

```java
i64[.] squares(i64 n) {
    i64[..>] r = [];
    for (i64 i = 1; i <= n; i++) {
        r +> i * i;
    }
    return r;
}

void main() {
    i64[.] sq = squares(4);
    sq[0] = 100;
    sq.sort();
    println("${sq} ${sq.len}");
}
```

```output
[4, 9, 16, 100] 4
```

- **A function returning `T[.]` builds the array where the caller keeps it**, so `sq` is
  written once ([DESIGN §3.3](../DESIGN.md#33-building-in-place-decided)).
- **Nothing can be added or removed**, and `clear` isn't allowed either:

```java
void main() {
    i64[.] frozen = [1, 2, 3];
    frozen +> 4;
}
```

```error
example.zn:3:5: error: `+>` needs an array that can insert at the end (`>` or `~` as its last slot), and `i64[.]` can't
```

- **`T[.*]` stores its length as a varint**: 1 byte up to 127 elements, 2 up to 16,383. It
  saves space in records that hold many short arrays.
- **A class with a `T[.]` or `T[.*]` field is variable-size**: its records are stored
  inline, one after another, so an array of them can be looped over but not indexed
  ([Memory and references](05-memory.md)):

```java
class Item {
    u8[.*] sku;
    i32 qty;
}

void main() {
    Item[..>] items = [];
    items +> new Item("SKU-1", 2);
    items +> new Item("SKU-22", 1);
    for (Item& it : items) {
        println("${it.sku} ${it.qty}");
    }
}
```

```output
SKU-1 2
SKU-22 1
```

```java
void main() {
    i64[.][..>] rows = [];
    rows +> [1, 2];
    println("${rows[0]}");
}
```

```error
example.zn:4:16: error: i64[.][..>] can't be indexed: its elements are variable-size, so it is only added to at its end, looped over and cleared
```

## Fixed and capped arrays

A number after the slots puts the elements inline, in whatever holds the array: a local, a
field, or an element of another array. No data stack is involved.

- **`T[.k]` is fixed**: exactly `k` elements, always. Its length isn't stored, `len` is the
  constant `k`, and its zero value is all zero bytes.
- **`T[..~k]` and `T[..>k]` are capped**: up to `k` elements, with a count as narrow as `k`
  allows, one byte up to 255.
- **Capped arrays with a start slot or a cursor slot** are ring buffers (`T[~.~k]`) and gap
  buffers (`T[~k]`, `T[.~.k]`), inline too (below).

```java
class Player {
    u8[..~16] name;
    i32[.3] scores;
    i64[~.~4] recent = [];

    mut void see(i64 x) {
        if (this.recent.len == 4) {
            @_ = this.recent -<;
        }
        @_ = this.recent +> x;
    }
}

void main() {
    Player p = new Player("ada", [7, 9, 8]);
    for (i64 i = 1; i <= 6; i++) {
        p.see(i * 10);
    }
    p.scores[1] = 10;
    println("${p.name} ${p.scores} ${p.scores.len} ${p.recent}");
}
```

```output
ada [7, 10, 8] 3 [30, 40, 50, 60]
```

### A capped array can be full

An insert into a full capped array fails with `status.FULL` and stores nothing. `push`,
`pushFront` and `insert` still return a reference, to the last element, so it is safe to
use ([DESIGN §4.7](../DESIGN.md#47-methods-decided)). An unread `FULL` stops the program,
as any unread failure does.

```java
void main() {
    i64[..~4] c = [1, 2];
    i64[..>] more = [3, 4, 5];
    u8 st = @c.pushAll(more);
    println("${c} ${status.name(st)}");
    c.pushAll(more[0:2]);
    i64& r @full = c.push(9);
    println("${c} ${status.name(full)}, and r is ${r}");
    for (i64 i = 0; i < 10; i++) {
        if (@(c +> i)) {
            println("full at ${i}");
            break;
        }
    }
    c +> 10;
}
```

```output
[1, 2] FULL
[1, 2, 3, 4] FULL, and r is 4
full at 0
```

```stderr exit=1
example.zn:15: `+>` failed with FULL, and nothing read its status
```

- **`pushAll` is all or nothing**: when the elements don't all fit, it stores none of them.
- **A literal longer than the cap doesn't compile** (above); **a slice or text that's
  longer stops the program** when it is converted, as a cast that doesn't fit does.
- **A capped `u8` array is text**, with `append`, which fails with `FULL` the same way
  ([Text](03-text.md)).
- **A cap must be at least 1**, so a full array always has a last element.
- **A capped array can go to a parameter typed by what it allows**, such as
  `i64[..>]& out`. The function gets a copy for the capped layout, in which a push can fail
  with `FULL` ([Arrays as parameters](#arrays-as-parameters)).

## Deques and arrays that change at the start

A `>` or `~` in the start slot allows `pushFront` (`+<`); a `<` or `~` allows `popFront`
(`-<`). On a data stack, an array that changes at both ends starts in the middle of its
reservation, so either end can grow.

```java
void main() {
    i64[~.~] q = [];
    q +> 1 +> 2;
    q +< 0;
    q.pushFront(-1);
    i64 front = q -<;
    i64 back = q ->;
    println("${q}, took ${front} and ${back}");
    for (i64 i = 0; i < 100000; i++) {
        q +> i;
        @_ = q -<;
    }
    println("after a long queue: ${q.len} left, ${q}");

    i64[>..] rev = [];
    rev +< 1 +< 2 +< 3;
    println("${rev} ${rev[0]}");

    i64[<.<] window = [1, 2, 3, 4, 5];
    @_ = window -<;
    @_ = window ->;
    println("window ${window}");
}
```

```output
[0, 1], took -1 and 2
after a long queue: 2 left, [99998, 99999]
[3, 2, 1] 3
window [2, 3, 4]
```

- **A reference follows its element.** After a front insert, it is the element at the next
  index along, and still reads the same value; nothing moved.
- **A capped deque (`T[~.~k]`) is a ring buffer.** One that changes only at the start
  (`T[>..k]`, `T[~..k]`) sits at the top of its buffer and never wraps.
- **`T[<.<]` is a window that only narrows**: filled when it is built, then popped from
  either end.
- **A reversed array on a data stack (`T[>..]`) can't be sliced** yet, as its elements
  aren't in order in one block (below, "Slices").

## Cursor arrays

A slot other than `.` in the middle gives the array a **cursor**: a position between two
elements, or at either end, where elements can be inserted (`+^`) and removed (`-^` for the
one before it, `^-` for the one after it). An array of `n` elements has `n + 1` cursor
positions, from 0 to `n`. Editing at the cursor costs O(1), and moving the cursor costs the
distance it moves.

```java
void main() {
    i64[.~.] c = [];
    c +^ 1 +^ 2 +^ 3 +^ 4;
    println("${c} cursor ${c.cursor}");
    c.seek(2);
    c +^ 20;
    println("${c} cursor ${c.cursor}");
    c.move(-10);
    println("move(-10) stops at ${c.cursor}");
    c.move(100);
    println("move(100) stops at ${c.cursor}");
    c.seekFromEnd(2);
    println("seekFromEnd(2) is ${c.cursor}");
    c.seekStart();
    u8 st = @c.seek(6);
    println("seek(6) fails with ${status.name(st)}, cursor still ${c.cursor}");
    i64& r = c.insertAt(1, 10);
    println("${c} cursor ${c.cursor}, r is ${r}");
    i64 gone = c.removeAt(0);
    println("${c} cursor ${c.cursor}, removed ${gone}");
    i64 after = c ^-;
    i64 none @e = c -^;
    println("${after}, then ${none} with ${status.name(e)}");
}
```

```output
[1, 2, 3, 4] cursor 4
[1, 2, 20, 3, 4] cursor 3
move(-10) stops at 0
move(100) stops at 5
seekFromEnd(2) is 3
seek(6) fails with POSITION, cursor still 0
[1, 10, 2, 20, 3, 4] cursor 2, r is 10
[10, 2, 20, 3, 4] cursor 0, removed 1
10, then 0 with EMPTY
```

The cursor API ([DESIGN §4.9](../DESIGN.md#49-cursor-api-decided)):

| Method | Meaning | Fails with |
|---|---|---|
| `cursor` | the current position, a property | |
| `seek(i)` | move to position `i`, 0 through `len` | `POSITION`, leaving the cursor where it was |
| `seekFromEnd(k)` | move to `len - k`; `seekFromEnd(0)` is the end | `POSITION` |
| `seekStart()` | `seek(0)` | |
| `move(n)` | move `n` positions, right for positive, clamped at the ends | |
| `insert(x)`, `+^` | insert before the cursor, which moves past it; returns a `T&` | `FULL` when capped |
| `removeBefore()`, `-^` | remove the element before the cursor (backspace) | `EMPTY` |
| `removeAfter()`, `^-` | remove the element after the cursor (delete) | `EMPTY` |
| `insertAt(i, x)` | `seek(i)`, then `insert(x)` | `POSITION`, `FULL` |
| `removeAt(i)` | `seek(i)`, then `removeAfter()`: removes element `i` | `POSITION`, `EMPTY` |

- **The cursor starts at 0**, after a literal too. `push` at the end leaves it where it is;
  `pushFront` moves it one along, so it stays before the same element. `clear` puts it
  back at 0.
- **There is no `seekEnd()`**: `seekFromEnd(0)` is the end. `a[^1]` counts elements from 1
  while `seekFromEnd(0)` counts gaps from 0.
- **`insertAt` and `removeAt` aren't random-access inserts.** Each moves the cursor first,
  at O(distance), or O(√n) with `$`.
- **Elements never move.** A removal leaves a hole that the next insert reuses, so a cursor
  array never holds more slots than the most elements it held at once
  ([DESIGN §4.5](../DESIGN.md#45-cursor-removal-and-hole-reuse-decided)). The order is kept
  in a separate index, which is why a read takes two loads.
- **Sorting a cursor array permutes its index**, not its elements, so a reference follows
  its element through a sort. On every other array, a reference keeps pointing at its slot:

```java
void main() {
    i64[..>] plain = [3, 1, 2];
    i64[.~.] cur = [3, 1, 2];
    i64& p = plain[0];
    i64& c = cur[0];
    plain.sort();
    cur.sort();
    println("p reads ${p}, c still reads ${c}");
    c = 30;
    println("${plain} ${cur}");
}
```

```output
p reads 1, c still reads 3
[1, 2, 3] [1, 2, 30]
```

- **A capped cursor array (`T[~8]`) is a gap buffer** inline in its parent. Its elements do
  move, so an insert into it ends references into it, like a removal:

```java
void main() {
    i64[~8] g = [1, 2, 3];
    i64& first = g[0];
    @_ = g +^ 0;
    println("${first}");
}
```

```error
example.zn:4:10: error: `+^` can move the elements of `g` while `first` may still refer into it
```

- **A cursor array on a data stack can't be sliced** or passed where a `string` or a `T[:]`
  is expected, since its elements aren't in order in one block. Slices of them are an open
  decision ([DESIGN §17](../DESIGN.md#17-open-decisions)). Capped gap buffers can be
  sliced (below).
- **A class can hold a cursor array only if it is a root**, since one takes two data stacks
  ([Classes, interfaces and generics](06-classes.md)). `examples/linebuf.zn` is a line
  editor built on one.

### Chunked cursor arrays: `$`

A `$` after the slots splits the cursor array's index into chunks of about √n entries, each
a circular buffer: a two-level tiered vector. Inserting or removing anywhere then costs
O(√n), and `seek` is free, since the cursor is only a number. Reads take three loads
instead of two. It measured faster than a plain cursor array from about 50,000 elements
edited at scattered positions, and 12–16 times faster at a million
([DESIGN §4.1](../DESIGN.md#41-type-grammar-decided)).

```java
void main() {
    u32[~$] big = [];
    for (i64 i = 0; i < 10; i++) {
        big +> (u32) i;
    }
    big.seek(5);
    big +^ 100;
    @_ = big.insertAt(0, 99);
    u32 x = big.removeAt(3);
    println("${big} len ${big.len} cursor ${big.cursor}, removed ${x}");
    big.sort();
    println("${big}");
}
```

```output
[99, 0, 1, 3, 4, 100, 5, 6, 7, 8, 9] len 11 cursor 3, removed 2
[0, 1, 3, 4, 5, 6, 7, 8, 9, 99, 100]
```

- **The operations are the same as without `$`**; only their costs change.
- **`$` can't be combined with a numeric cap**: capped arrays are small enough for one gap
  buffer to win.

## Operators

The sign says insert (`+`) or remove (`-`); the symbol says where: `<` the start, `^` the
cursor, `>` the end ([DESIGN §4.8](../DESIGN.md#48-operators-decided)).

| Operator | Method | Needs | Returns | Fails with |
|---|---|---|---|---|
| `a +> x` | `push(x)` | `>` or `~` as the last slot | the array (the method: a `T&`) | `FULL` when capped |
| `a +< x` | `pushFront(x)` | `>` or `~` as the first slot | the array (the method: a `T&`) | `FULL` when capped |
| `a +^ x` | `insert(x)` | `>` or `~` as the middle slot | the array (the method: a `T&`) | `FULL` when capped |
| `a ->` | `pop()` | `<` or `~` as the last slot | the removed value | `EMPTY` |
| `a -<` | `popFront()` | `<` or `~` as the first slot | the removed value | `EMPTY` |
| `a -^` | `removeBefore()` | `<` or `~` as the middle slot | the removed value | `EMPTY` |
| `a ^-` | `removeAfter()` | `<` or `~` as the middle slot | the removed value | `EMPTY` |

```java
void main() {
    i64[~] d = [];
    d +> 1 +> 2 +> 3;
    d +< 0 +< -1;
    d.seek(2);
    d +^ 100 +^ 101;
    println("${d} cursor ${d.cursor}");
    i64 end = d ->;
    i64 start = d -<;
    i64 before = d -^;
    i64 after = d ^-;
    println("${end} ${start} ${before} ${after}, leaving ${d} cursor ${d.cursor}");
    i64& r = d.push(7);
    r *= 10;
    var copy = d.push(8);
    copy = 0;
    println("${d}");
}
```

```output
[-1, 0, 100, 101, 1, 2, 3] cursor 4
3 -1 101 1, leaving [0, 100, 2] cursor 2
[0, 100, 2, 70, 8]
```

- **Insert operators return the array, so they chain**: `a +> 1 +> 2 +> 3` appends in
  order. The methods return a reference to the new element instead.
- **Remove operators are postfix** and return the removed value, so they end a chain.
- **`var` copies**: `var copy = d.push(8)` is a copy of the new element, so writing to it
  changes nothing in `d`. `i64& r = d.push(7)` refers to it.
- **Each operator needs what its method needs**, and the error says which slot:

```java
void main() {
    i64[..>] grow = [];
    i64[..~] stack = [1, 2];
    i64 x = grow.pop();
    stack +< 0;
}
```

```error
example.zn:4:18: error: `pop` needs an array that can remove at the end (`<` or `~` as its last slot), and `i64[..>]` can't
example.zn:5:5: error: `+<` needs an array that can insert at the start (`>` or `~` as its first slot), and `i64[..~]` can't
```

- **`a^-b` lexes as `a ^- b`**, the cursor removal, so an XOR with a negated value needs a
  space: `a ^ -b`.

```java
void main() {
    i64 a = 5;
    i64 b = 3;
    i64 c = a^-b;
}
```

```error
example.zn:4:14: error: `^-` removes the element after the cursor
```

## Length, clearing and truncating

| Member | Needs | Meaning |
|---|---|---|
| `len` | any array or slice | the number of elements, an `i64` property; the constant `k` for `T[.k]` |
| `clear()` | an array that can insert or remove | removes every element; on a data stack, one store |
| `truncate(n)` | remove at the end | keeps the first `n` elements; an `n` at or past `len` changes nothing |

```java
void main() {
    i64[..~] xs = [3, 1, 4, 1, 5];
    xs.truncate(3);
    println("${xs} ${xs.len}");
    xs.truncate(10);
    println("${xs}");
    xs.clear();
    println("${xs.len}");
}
```

```output
[3, 1, 4] 3
[3, 1, 4]
0
```

- **`len` is a property**, not a method: `xs.len()` is an error that says `i64[..~]` has no
  method `len`.
- **`clear` and `truncate` are removals** under rule R, as `pop` is
  ([Memory and references](05-memory.md)).
- **A negative `n` empties the array**, with no failure.

## Indexing

`a[i]` reads or writes element `i`, counting from 0. Three more forms count differently
([DESIGN §4.10](../DESIGN.md#410-indexing-decided)):

| Form | Element | Out of range |
|---|---|---|
| `a[i]` | `i` | `i < 0` or `i >= len` |
| `a[^k]` | `len - k`, so `a[^1]` is the last | `k < 1` or `k > len` |
| `a[%i]` | `i` modulo `len`, true modulo, so `a[%-1]` is the last | an empty array |
| `a[\|i]` | `i` clamped to the first or last element | an empty array |
| `a.getOr(i, fallback)` | `i`, or `fallback` when `i` is out of range | never fails |

```java
void main() {
    i64[..>] a = [10, 20, 30, 40];
    println("${a[0]} ${a[^1]} ${a[^4]} ${a[%5]} ${a[%-1]} ${a[|99]} ${a[|-5]} ${a.getOr(9, -1)}");
    a[%6] = 1;
    a[|100] = 2;
    a[^2] += 5;
    println("${a}");
    i64 bad @e = a[4];
    println("${bad} ${status.name(e)}");
    i64 neg @e2 = a[-1];
    println("${neg} ${status.name(e2)}");
    u8 small = 2;
    u64 big = 3;
    println("${a[small]} ${a[big]}");
}
```

```output
10 40 10 20 40 40 10 -1
[10, 20, 6, 2]
0 RANGE
0 RANGE
6 2
```

- **An index out of range fails with `RANGE`**, and the value is the zero value. With
  nothing to read the status, the program stops at that line, naming the index and the
  length ([Errors and statuses](07-errors.md)):

```java
i64 third(i64[:] xs) {
    return xs[2];
}

void main() {
    i64[..>] a = [1, 2];
    println("${third(a)}");
}
```

```stderr exit=1
example.zn:2: index 2 is out of range for length 2
```

- **There are no negative indexes**: `a[-1]` is out of range, so a computed index that goes
  negative never wraps by accident. Write `a[^1]` or `a[%-1]`.
- **An index can be any integer type**, signed or unsigned, with no cast: the check rejects
  any value past the length.
- **`^k` is bracket syntax**, not a value: it can't be stored or passed. A constant `a[^0]`
  is a compile error that says `a[^1]` is the last element.
- **`%` and `|` work for writes too.** Both still fail on an empty array.
- **In a counted loop, one test before the loop often stands for every check inside it**,
  so indexing in a loop such as `for (i64 i = 0; i < a.len; i++)` costs no check per
  element.
- **An array of variable-size values can't be indexed** (above, "Frozen arrays").

## Slices

`a[i:j]` is a **slice**: a read-only view of the elements from `i` up to, but not
including, `j`. It is a pointer and a length, and copies nothing. Its type is `T[:]`, and
`string` is the name of `u8[:]`.

```java
i64 total(i64[:] xs) {
    i64 t = 0;
    for (i64 x : xs) {
        t += x;
    }
    return t;
}

void main() {
    i64[..>] a = [5, 1, 4, 2, 3, 9, 0];
    i64[..>] b = [7, 8];
    println("${a[1:3]} ${a[4:]} ${a[:2]} ${a[^2:]} ${a[1:^1]} ${a[:]}");
    println("${total(a)} ${total(a[2:4])}");
    i64[:] tail = a[2:];
    println("${tail.len} ${tail[0]} ${tail[^1]}");
    tail = b;
    println("${tail}");
}
```

```output
[1, 4] [3, 9, 0] [5, 1] [9, 0] [1, 4, 2, 3, 9] [5, 1, 4, 2, 3, 9, 0]
24 6
5 4 0
[7, 8]
```

- **`a[i:]` runs to the end, `a[:j]` starts at the beginning, and `a[:]` is the whole
  array.** `^` counts from the end in either bound: `a[1:^1]` leaves out the first and last
  elements.
- **A bound out of range, or `i > j`, stops the program**:

```java
void main() {
    i64[..>] a = [1, 2, 3];
    i64 j = 5;
    i64[:] s = a[1:j];
}
```

```stderr exit=1
example.zn:4: slice 1:5 is out of range for length 3
```

- **An array converts to a slice wherever one is expected**: as an argument, as a method's
  receiver, in a declared `T[:]` or `string`, in a `return`, and beside `==` when the other
  side is a slice. So `total(a)` needs no `a[:]`, and a text method works on a `u8` array.
  The view is of the array, as `a[:]` would be.
- **`=` on a `T[:]` makes it view something else**, since a slice is a value; it never
  copies elements. Assigning to an element of one is an error:

```java
void main() {
    i64[..>] a = [1, 2, 3, 4];
    i64[:] s = a[0:2];
    s[0] = 9;
}
```

```error
example.zn:4:5: error: can't assign to this: an `i64[:]` is a read-only view
```

- **A slice can't outlive its array**, and a removal from the array can't happen while a
  slice of it may still be used ([Memory and references](05-memory.md)).
- **A slice of a temporary**, such as `letters()[1:]`, is copied to a scratch stack and
  lasts to the end of its statement.
- **Slice bounds are shift-level expressions**, so a conditional keeps its own `:`:
  `a[c ? i : j]` is an index, and a bound that uses a comparison, `&`, `|` or `?:` needs
  parentheses: `a[(c ? i : j):]`.

### Which arrays can be sliced

An array whose elements sit in order in one block can be sliced and converted to a slice:
every kind except cursor arrays, chunked arrays and reversed arrays (`T[>..]`) on data
stacks.

```java
void main() {
    i64[~] c = [1, 2, 3];
    i64[:] s = c[0:2];
}
```

```error
example.zn:3:16: error: `i64[~]` doesn't keep its elements in order in one block, so it can't be sliced
```

**Capped ring and gap buffers can be sliced**: when a slice is taken and the elements are
split, all of them are joined first. A ring rotates so its first element is at the start,
and a gap moves to whichever end moves fewer elements. Neither the order nor the cursor
changes, and later slices move nothing until the next edit, so every slice taken since the
last edit stays right.

```java
void main() {
    i64[~.~4] q = [1, 2, 3, 4];
    @_ = q -<;
    @_ = q -<;
    @_ = q +> 5;
    @_ = q +> 6;
    i64[:] mid = q[1:^1];
    println("across the wrap ${mid}, the ring is ${q}");

    u8[~16] g = "hello world";
    @_ = g.seek(5);
    @_ = g +^ ',';
    string all = g;
    println("'${all}' cursor ${g.cursor}, ${g[0:3]} ${g[7:]}");
}
```

```output
across the wrap [4, 5], the ring is [3, 4, 5, 6]
'hello, world' cursor 6, hel world
```

## Writable views: `T[:]&`

A `T[:]` is read-only. A **writable view**, `T[:]&`, can write its elements, though not
change its length, as `&` elsewhere means "may change it". A writable view is taken of an
array the code may change, and an array converts to one wherever one is expected, so
`a[1:5].sort()` sorts part of an array in place.

```java
void twice(i64[:]& xs) {
    for (i64& x : xs) {
        x *= 2;
    }
}

void main() {
    i64[..>] a = [5, 1, 4, 2, 3, 9, 0];
    a[1:5].sort();
    println("${a}");
    twice(a[0:2]);
    println("${a}");
    a[5:7] = a[0:2];
    println("${a}");
    i64[:]& w = a[0:3];
    w[0] = -1;
    w .= a[4:7];
    w[0] = -2;
    println("${a} ${w}");
    a[0:3].fill(7);
    a[2:4].reverse();
    twice(a);
    println("${a}");
}
```

```output
[5, 1, 2, 3, 4, 9, 0]
[10, 2, 2, 3, 4, 9, 0]
[10, 2, 2, 3, 4, 10, 2]
[-1, 2, 2, 3, -2, 10, 2] [-2, 10, 2]
[14, 14, 6, 14, -4, 20, 4]
```

- **`=` copies elements into the view**, as `=` through a reference writes its target. The
  lengths must match, or the program stops. A copy between overlapping parts of one array
  gives the right result: `a[1:5] = a[0:4]` shifts four elements right by one.
- **`.=` rebinds the view** to another range, as it rebinds a reference.
- **Every in-place method works on a writable view**: the sorts, `reverse`, `swap` and
  `fill`.
- **A writable view needs an array the code may change**: a local, a `&` parameter, or a
  field inside a `mut` method. Not a plain parameter, which is a read-only copy, a `T[:]`,
  or a string literal, which lives in read-only memory:

```java
void zero(i64[:]& xs) {
    xs.fill(0);
}

void clearAll(i64[..>] a) {
    zero(a);
}

void main() {
    i64[..>] a = [1];
    clearAll(a);
}
```

```error
example.zn:6:10: error: can't pass this to `zero`, which may change it: `a` is a plain parameter, which is a read-only copy
```

```java
void main() {
    i64[..>] a = [1, 2, 3, 4];
    a[0:2] = a[1:4];
}
```

```stderr exit=1
example.zn:3: the lengths differ, so the copy doesn't fit the view
```

- **A writable view can't change its length**, so `+>` on one is an error that says a slice
  can't change its length.

## For-each

`for (T x : a)` visits every element in order, copying each into `x`. `for (T& x : a)`
refers to each in place, so the loop can change the elements. `var` works in both forms.

```java
void main() {
    i64[..>] a = [1, 2, 3];
    for (i64 x : a) {
        if (x < 3) {
            a +> x * 10;
        }
    }
    println("${a}");
    for (i64& x : a) {
        x += 1;
    }
    println("${a}");
    for (var x : a) {
        x = 0;
    }
    println("${a}");
    i64 n = 0;
    for (i64 x : a[1:3]) {
        n += x;
    }
    println("${n}");
}
```

```output
[1, 2, 3, 10, 20]
[2, 3, 4, 11, 21]
[2, 3, 4, 11, 21]
7
```

- **Appending inside the loop is safe**: elements never move, and the loop reads `len` again
  on each step, so it visits what was appended. Above, `10` and `20` were visited too, and
  appended nothing more.
- **A copy is a copy**: assigning to `x` in a by-value loop changes nothing in the array.
- **Removing from the array inside its own loop is an error**, by value or by reference:

```java
void main() {
    i64[..~] a = [1, 2, 3];
    for (i64 x : a) {
        if (x == 2) {
            i64 y @_ = a.pop();
        }
    }
}
```

```error
example.zn:5:24: error: `pop` removes from `a` inside a for-each over it
```

  Remove afterwards, use `retain` (below), or use a repeat loop such as
  `while (i64 x : a ->)`.
- **A read-only slice gives copies only**: `for (i64& x : xs)` over a `T[:]` is an error
  that says to drop the `&`. Over an array, a writable view, or a plain array parameter, a
  by-reference loop works; over a plain parameter, the references are read-only.
- **Every kind of array can be looped over**, cursor and chunked arrays included, in
  logical order. Maps, sets, pools and iterators have for-each loops too
  ([Maps, sets, pools and iterators](09-collections.md)).

## Arrays as parameters

A plain parameter is a read-only copy, which the compiler passes by pointer when it can. A
function that changes an array takes it by reference, with `&`
([Memory and references](05-memory.md)).

- **A `T[:]` parameter takes any array that can be sliced**, or a slice: it can read but
  not change the elements.
- **A `T[:]&` parameter takes an array the caller may change**, or a writable view: it can
  write and reorder the elements, not add or remove them.
- **A parameter typed by its slots takes a capability, not a layout**: `i64[..>]& out`
  accepts an array that can at least be appended to, and the function can only append.

```java
void fill(i64[..>]& out, i64 n) {
    for (i64 i = 0; i < n; i++) {
        out +> i;
    }
}

void bump(i64[..~4]& small) {
    @_ = small +> 100;
}

void main() {
    i64[..>] a = [];
    i64[..~] b = [];
    i64[~.~] c = [];
    fill(a, 3);
    fill(b, 2);
    fill(c, 1);
    i64[..~4] d = [1];
    bump(d);
    println("${a} ${b} ${c} ${d}");
}
```

```output
[0, 1, 2] [0, 1] [0] [1, 100]
```

- **Such a function is compiled once per layout passed to it**: a capped, fixed, cursor or
  double-ended array gets a copy of the function in which the parameter has its own type,
  as do the functions it passes the parameter on to. In a copy for a capped array, a push
  can fail with `FULL`, and stops the program unless something reads its status.
- **Except a capped ring or gap buffer**, such as `i64[~8]` or `i64[~.~4]`, for a parameter
  whose elements stay where they are, such as `i64[..>]&`: an insert, a sort or a slice
  can move the buffer's elements, and the function is checked for its parameter's type.
  Pass a slice, or give the parameter the buffer's type.
- **Not yet** in such a copy: binding a reference to the parameter, assigning it, or
  capturing it in a lambda passed to a function of the program's (one passed to a built-in
  method, as `sort`'s comparator, is fine). Each is reported where it is written. A library
  function whose C fills the array, such as `stdin.read`, takes only an array on a data
  stack, and says so at the call.
- **A plain array parameter can't be changed**, and the error says to add `&`:

```java
void add(i64[..>] xs) {
    xs +> 1;
}

void main() {
    i64[..>] a = [];
    add(a);
}
```

```error
example.zn:2:5: error: can't use `+>` here, since it inserts into the array: `xs` is a plain parameter, which is a read-only copy
```

## Sorting

Every sort works in place, on an array or a writable view. Most come in three forms:
`()` uses the elements' natural order and needs them to be `ordered`; `(less)` takes a
comparator, a lambda that says whether its first argument goes before its second; and
`By(key)` takes a key function and compares the keys in their natural order
([STDLIB §7.1](../STDLIB.md#71-methods-on-arrays-v1)).

| Method | What it does |
|---|---|
| `sort()`, `sort(less)`, `sortBy(key)` | ascending |
| `sortDescending()`, `sortByDescending(key)` | largest first |
| `stableSort()`, `stableSort(less)`, `stableSortBy(key)` | ascending, keeping equal elements in their order |
| `radixSort()`, `radixSortBy(key)` | ascending by a number or string key, stable |
| `partialSort(k)`, `partialSort(k, less)`, `partialSortBy(k, key)` | the `k` smallest first, in order; the rest in no particular order |
| `T& selectNth(k)`, `selectNth(k, less)`, `selectNthBy(k, key)` | puts at `k` what a sort would put there, smaller before, larger after, and returns a reference to it |
| `bool isSorted()`, `isSorted(less)`, `isSortedBy(key)` | whether the array is in order |
| `i64[..>] sortedIndexes()`, `sortedIndexes(less)`, `sortedIndexesBy(key)` | the indexes in sorted order, leaving the array as it is |
| `reverse()` | reverses the order |
| `swap(i, j)` | swaps two elements |
| `fill(x)` | sets every element to `x` |

```java
class Person {
    string name;
    i64 age;
}

void main() {
    i64[..~] xs = [5, 3, 9, 1, 7, 3];
    xs.sort();
    println("sort ${xs} ${xs.isSorted()}");
    xs.sortDescending();
    println("descending ${xs} ${xs.isSorted()} ${xs.isSorted((i64 a, i64 b) => a > b)}");
    xs.sort((i64 a, i64 b) => a % 3 < b % 3);
    println("by remainder ${xs}");

    Person[..>] ps = [];
    ps +> new Person("cy", 30) +> new Person("al", 25) +> new Person("bo", 30) +> new Person("di", 25);
    ps.sortBy((Person p) => p.name);
    ps.stableSortBy((Person p) => p.age);
    for (Person& p : ps) {
        print("${p.name}:${p.age} ");
    }
    println("");
    ps.radixSortBy((Person p) => p.name);
    for (Person& p : ps) {
        print("${p.name} ");
    }
    println("");

    i64[..>] big = [9, 2, 7, 4, 5, 1, 8];
    big.partialSort(3);
    println("the three smallest ${big[0:3]}");
    i64& median = big.selectNth(3);
    println("the median ${median}, at ${big[3]}");
    big.reverse();
    big.swap(0, 1);
    println("${big}");
}
```

```output
sort [1, 3, 3, 5, 7, 9] true
descending [9, 7, 5, 3, 3, 1] false true
by remainder [9, 3, 3, 7, 1, 5]
al:25 di:25 bo:30 cy:30 
al bo cy di 
the three smallest [1, 2, 4]
the median 5, at 5
[8, 9, 7, 5, 4, 2, 1]
```

- **Sorting by a second key and then, stably, by the first** gives an order by both: above,
  by age, and by name within each age.
- **A comparator should be a strict order**: `<`, not `<=`. One that isn't still gives some
  order, and never reads outside the array.
- **`By` forms call the key about n log n times**, so a key should be cheap: a field, or
  arithmetic on fields.
- **Lambdas passed to these methods are inlined**, so a sort with a comparator costs what a
  handwritten loop would, and the lambda may use the function's locals
  ([Functions and lambdas](08-functions.md)).
- **Today every sort, `radixSort` included, is a stable merge sort**, and `partialSort`
  keeps a heap of the smallest, O(n log k). Only the `stableSort` and `radixSort` forms
  promise stability: the design has `sort` as an unstable pattern-defeating quicksort, and
  `radixSort` as a true radix sort, which a later compiler may use.
- **`partialSort(k)` with `k` past the length sorts everything.**
- **Floats sort with NaN last**, in both directions, so a NaN can't break a sort.
- **Strings sort byte by byte**, so `"File1"` comes before `"file10"`, and `"file10"`
  before `"file2"`. For file names and version numbers, sort with `compareNatural`
  ([Text](03-text.md)):

```java
void main() {
    string[..>] names = ["file10", "file2", "File1"];
    names.sort();
    println("${names}");
    names.sort((string a, string b) => a.compareNatural(b) < 0);
    println("${names}");
    f64[..>] f = [2.0, f64.NAN, -1.0, 0.5];
    f.sort();
    println("${f}");
    f.sortDescending();
    println("${f}");
}
```

```output
[File1, file10, file2]
[File1, file2, file10]
[-1.0, 0.5, 2.0, NaN]
[2.0, 0.5, -1.0, NaN]
```

- **`sortedIndexes` leaves the elements where they are**, so it works on a read-only slice,
  and on records too large to move cheaply:

```java
class Row {
    string key;
    i64 n;
}

void show(i64[:] xs) {
    for (i64 i : xs.sortedIndexes()) {
        print("${xs[i]} ");
    }
    println("");
}

void main() {
    i64[..>] xs = [5, 2, 8, 1, 9];
    show(xs);
    println("${xs.sortedIndexes()} ${xs.sortedIndexes((i64 a, i64 b) => a > b)} ${xs}");
    Row[..>] rows = [];
    rows +> new Row("b", 2) +> new Row("a", 1) +> new Row("c", 0);
    i64[..>] byKey = rows.sortedIndexesBy((Row r) => r.key);
    for (i64 i : byKey) {
        print("${rows[i].key}${rows[i].n} ");
    }
    println("");
}
```

```output
1 2 5 8 9 
[3, 1, 0, 2, 4] [4, 2, 0, 1, 3] [5, 2, 8, 1, 9]
a1 b2 c0 
```

- **The natural order needs `ordered` elements**: numbers, `bool`, enums and strings. A
  class needs a comparator or a key:

```java
class P {
    bool b;
}

void main() {
    P[..>] ps = [];
    ps.sort();
    ps.radixSortBy((P p) => p.b);
}
```

```error
example.zn:7:5: error: `sort` needs elements that are `ordered`, and P isn't
example.zn:8:5: error: a radix sort's key is a number or a string, and this is bool
```

- **A sort needs an array or a writable view**; on a `T[:]` it is an error that says so.
- **A reference keeps pointing at its slot** through a sort, and sees whatever is there
  afterwards; on a cursor array it follows its element (above, "Cursor arrays"). Nothing is
  freed, so a sort isn't a removal under rule R, except on a capped ring or gap buffer,
  which a sort may first join into one block.
- **Not yet:** `shuffle(rng)`, and the natural order of a class that `implements ordered`
  with a `compareTo` method: both type-check, and the C backend doesn't generate code for
  them yet ([What isn't there yet](15-not-yet.md)).

## Searching

| Method | Result | Fails with |
|---|---|---|
| `indexOf(x)` | the first index of an element `== x` | `MISSING` |
| `contains(x)` | whether an element `== x` | |
| `findIndex(pred)` | the first index where `pred` is true | `MISSING` |
| `any(pred)`, `all(pred)` | whether `pred` is true for any, or for all | |
| `count(pred)` | how many elements `pred` is true for | |

```java
class Row {
    string key;
    i64 num;
}

void main() {
    i64[..>] xs = [4, 8, 1, 9, 8];
    i64 i @e = xs.indexOf(8);
    i64 j @e2 = xs.indexOf(5);
    println("${i} ${e}, ${j} ${status.name(e2)}, ${xs.contains(9)} ${xs.contains(2)}");
    i64 limit = 5;
    i64 k @_ = xs.findIndex((i64 x) => x > limit);
    println("${k} ${xs.any((i64 x) => x > 8)} ${xs.all((i64 x) => x > 0)} ${xs.count((i64 x) => x == 8)}");

    Row[..>] rows = [];
    rows +> new Row("pear", 3) +> new Row("fig", 7);
    i64 at @_ = rows.findIndex((Row r) => r.key == "fig");
    i64 none @gone = rows.findIndex((Row r) => r.key == "plum");
    println("fig at ${at}, plum ${none} ${status.name(gone)}");
}
```

```output
1 0, 0 MISSING, true false
1 true true 2
fig at 1, plum 0 MISSING
```

- **`indexOf` and `contains` compare with `==`.** Today they work on arrays of numbers and
  `bool`; on strings, enums and classes they type-check but the C backend doesn't generate
  them yet. `findIndex` and `any` with a lambda work for every element type:
  `words.any((string w) => w == "x")`.
- **Not yet:** `binarySearch`, `lowerBound`, `upperBound` and their `By` forms
  ([What isn't there yet](15-not-yet.md)).

## Building new values: `filter`, `map`, `fold`, `sum`, `min`, `max`

| Method | Result | Fails with |
|---|---|---|
| `filter(keep)` | a new `T[..>]` of the elements `keep` is true for, in order | |
| `map(f)` | a new `U[..>]` of what `f` returns for each element | |
| `fold(init, f)` | `f(f(init, a[0]), a[1])` and so on: one value from all of them | |
| `sum()` | the total of a number array; overflow stops the program | |
| `min()`, `max()` | the smallest or largest element | `EMPTY` on an empty array |

```java
void main() {
    i64[..>] xs = [5, 2, 8, 1, 9];
    i64[..>] big = xs.filter((i64 x) => x > 4);
    f64[..>] halves = xs.map((i64 x) => (f64) x / 2.0);
    i64 total = xs.fold(0, (i64 acc, i64 x) => acc + x);
    println("${big} ${halves} ${total} ${xs.sum()} ${xs.min()} ${xs.max()}");
    var hex = xs.map((i64 x) => x.hex());
    println("${hex[2]} ${xs.map((i64 x) => "<${x}>")}");
    i64[..>] empty = [];
    i64 m @st = empty.min();
    println("${m} ${status.name(st)} ${empty.sum()}");
}
```

```output
[5, 8, 9] [2.5, 1.0, 4.0, 0.5, 4.5] 25 25 1 9
8 [<5>, <2>, <8>, <1>, <9>]
0 EMPTY 0
```

- **`filter` and `map` build their array where the result goes**, as a function returning
  one does: in a new variable's storage, or on a scratch stack for a temporary, which lasts
  to the end of the statement.
- **Text a lambda builds lives only for the statement**: printing
  `xs.map((i64 x) => "<${x}>")` works, and keeping it in a `string[..>]` is an error
  ([Text](03-text.md)). A `map` that returns views of text that lives longer, such as
  `rows.map((Row r) => r.key)`, can be kept.
- **The elements, and what `map` returns, are values a C assignment copies**: not arrays,
  variable-size records, values with links, or classes with a growable field.
- **`sum` needs numbers, and `min` and `max` `ordered` elements.** `sum` of an empty array
  is 0.
- **On floats, `min()` and `max()` put NaN last**, as sorting does: `max()` of an array
  holding a NaN is NaN, and `min()` is the least of the rest. On strings they compare text.
- **Not yet:** `minIndexBy` and `maxIndexBy` ([What isn't there yet](15-not-yet.md)).

## Editing: `pushAll`, `retain`, `dedup`

| Method | Needs | Meaning |
|---|---|---|
| `pushAll(xs)` | insert at the end | appends every element of a slice; `FULL`, storing none, on a capped array without room |
| `retain(keep)` | remove at the end | keeps the elements `keep` is true for, in order |
| `dedup()` | remove at the end | removes each element equal to the one before it |
| `truncate(n)` | remove at the end | keeps the first `n` (above) |

```java
void main() {
    i64[..~] ys = [3, 3, 1, 1, 1, 2, 3];
    ys.dedup();
    println("${ys}");
    i64 limit = 1;
    ys.retain((i64 y) => y != limit);
    println("${ys}");
    i64[..>] xs = [5, 2];
    ys.pushAll(xs);
    ys.pushAll(xs[0:1]);
    println("${ys}");
    string[..~] words = ["b", "a", "c", "a"];
    words.sort();
    words.dedup();
    println("${words}");
}
```

```output
[3, 1, 2, 3]
[3, 2, 3]
[3, 2, 3, 5, 2, 5]
[a, b, c]
```

- **`dedup` removes only adjacent duplicates**, so sort first to remove every duplicate.
- **`retain` and `dedup` are removals** under rule R ([Memory and references](05-memory.md)).

## Chunks and windows

`chunks(n)` and `windows(n)` are iterators over slices of an array
([Maps, sets, pools and iterators](09-collections.md)): `chunks` gives consecutive slices
of `n` elements, the last one shorter, and `windows` every run of `n` consecutive
elements, none when the array is shorter than `n`. Each slice is a view into the array.

```java
void main() {
    i64[..>] xs = [1, 2, 3, 4, 5, 6, 7];
    for (i64[:] c : xs.chunks(3)) {
        print("${c} ");
    }
    println("");
    for (i64[:] w : xs.windows(5)) {
        print("${w} ");
    }
    println("");
    i64 n = 0;
    for (i64[:] w : xs.windows(9)) {
        n++;
    }
    println("${n} windows of 9");
}
```

```output
[1, 2, 3] [4, 5, 6] [7] 
[1, 2, 3, 4, 5] [2, 3, 4, 5, 6] [3, 4, 5, 6, 7] 
0 windows of 9
```

- **A size below 1 fails with `INVALID`**, and the iterator gives nothing.
- **They need an array that can be sliced**, so not a cursor array.

## Comparing arrays

`==` and `!=` compare two arrays, or an array and a slice or literal, element by element.
The kinds may differ; the element types must be the same.

```java
void main() {
    i64[..>] a = [1, 2, 3];
    i64[..~] b = [1, 2, 3];
    i64[.3] c = [1, 2, 4];
    println("${a == b} ${a != c} ${a[0:2] == c[0:2]} ${a == [1, 2, 3]}");
}
```

```output
true true true true
```

- **Only strings are ordered with `<`**; other arrays have `==` and `!=` only:

```java
void main() {
    i64[..>] a = [1, 2, 3];
    i64[.3] c = [1, 2, 4];
    u8[..>] bytes = [];
    println("${a < c} ${a == bytes}");
}
```

```error
example.zn:5:16: error: only strings can be ordered with `<`; these are i64[..>] and i64[.3]
example.zn:5:25: error: can't compare i64[..>] with u8[..>]
```

- **Elements compare with `==`**, so arrays of strings compare their text, wherever it is,
  and arrays of floats follow IEEE 754: `[0.0] == [-0.0]`, and an array holding a NaN
  isn't equal to itself.

```java
void main() {
    u8[..>] one = [];
    one.append("ab");
    u8[..>] two = [];
    two.append("ab");
    string[.] xs = [one[:]];
    string[.] ys = [two[:]];
    f64[.] nan = [f64.NAN];
    f64[.] zero = [0.0];
    f64[.] negZero = [-0.0];
    println("${xs == ys} ${nan == nan} ${zero == negZero}");
}
```

```output
true false true
```

- **Not yet:** `==` on arrays of classes, which the C backend refuses, as it does `==` on
  class values ([What isn't there yet](15-not-yet.md)).

## Arrays of classes

An array of a class holds its values inline, one after another. `push(new Item(...))`
builds the value in the array itself; `items[i].field` reads or writes a field in place; and
`Item& it = items[i]` refers to an element.

```java
class Item {
    string name;
    i64 qty;
}

void main() {
    Item[..>] items = [];
    items +> new Item("bolt", 10) +> new Item("nut", 3);
    items[1].qty += 4;
    Item& bolt = items[0];
    bolt.qty--;
    Item copy = items[1];
    copy.qty = 0;
    for (Item& it : items) {
        println("${it.name} ${it.qty}");
    }
    items.sortBy((Item it) => it.qty);
    println("${items[0].name} first");
}
```

```output
bolt 9
nut 7
nut first
```

- **A variable of a class type holds a copy**: `Item copy = items[1]` copies the element,
  so changing `copy` leaves the array alone. `Item&` refers to it instead
  ([Memory and references](05-memory.md)).
- **References into an array that can only grow stay valid** as it grows, and may be kept
  in fields. References into one that can shrink can be kept only in locals and parameters,
  and a removal can't happen while one may still be used. Which references survive which
  changes is in [Memory and references](05-memory.md):

```java
class Node {
    i64 v;
    Node&? next;
}

Node[..~] nodes = [];

void main() {
    Node& a = nodes.push(new Node(1, null));
    Node& b = nodes.push(new Node(2, null));
    a.next .= b;
}
```

```error
example.zn:11:5: error: a reference into `nodes`, which can remove elements, can be kept only in a local variable or a parameter
```

  With `Node[..>] nodes`, which only grows, the same program compiles.
- **An array of arrays holds frozen or capped inner arrays**, which can't grow once pushed:
  `u8[..~16][..>]` is an indexable list of short names, and `i64[.3][..>]` of triples. An
  array whose elements are still growing isn't possible; build each inner array in a
  scratch array first, then push it.

```java
void main() {
    u8[..~8][..>] names = [];
    names +> "ann";
    names +> "bob";
    names[0].append("ie");
    i64[.3][..>] rows = [];
    rows +> [1, 2, 3];
    rows +> [];
    rows[1][2] = 9;
    println("${names[0]} ${names[1]} ${names.len} ${rows[0]} ${rows[1]}");
}
```

```output
annie bob 2 [1, 2, 3] [0, 0, 9]
```

## Printing arrays

In `${...}`, an array of numbers, `bool`s or strings prints as a list, and a `u8` array as
text ([Text](03-text.md)).

```java
void main() {
    i64[..>] xs = [1, 2, 3];
    bool[..>] bs = [true, false];
    string[..>] ss = ["a", "b"];
    u8[..>] text = [104, 105];
    u16[..>] wide = [104, 105];
    f64[..>] fs = [0.1, 2.0];
    println("${xs} ${bs} ${ss} ${text} ${wide} ${fs} ${xs[1:]}");
}
```

```output
[1, 2, 3] [true, false] [a, b] hi [104, 105] [0.1, 2.0] [2, 3]
```

- **Strings in a list print without quotes**, so `["a", ""]` prints as `[a, ]`.
- **Not yet:** printing an array of arrays, of enums or of classes. The checker accepts
  them, and the C backend doesn't generate them yet; print the elements instead.

## What isn't there yet

These are designed and type-check, but the C backend doesn't generate code for them yet,
or the compiler handles them differently from the design. Chapter 15,
[What isn't there yet](15-not-yet.md), collects them with everything else.

- `binarySearch`, `lowerBound`, `upperBound` and their `By` forms; `minIndexBy`,
  `maxIndexBy`; `shuffle`; `indexOf` and `contains` on arrays of strings, enums and
  classes.
- Heaps: `heapPush`, `heapPop` and `heapify` on `T[..~]` arrays
  ([STDLIB §7.2](../STDLIB.md#72-heaps-v1)).
- Sorting a class by the `compareTo` of `implements ordered`.
- `==` on arrays of classes, as on classes.
- Printing arrays of arrays, enums or classes.
- The lint that warns about capabilities an array's type allows and the code never uses,
  such as a cursor slot never moved ([DESIGN §4.3](../DESIGN.md#43-layouts-the-compiler-picks-decided)).
- Slices of cursor, chunked and reversed arrays on data stacks, and varint lengths on
  arrays that can grow (`T[..>*]`), are open decisions
  ([DESIGN §17](../DESIGN.md#17-open-decisions)).
