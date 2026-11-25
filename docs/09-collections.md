# 9. Maps, sets, pools and iterators

Besides arrays, Zinkite has three built-in collections: `hashmap<K, V>`, `hashset<T>` and
`pool<T>`. Like arrays they need no `new` and no import: each is created with `[]`, lives
on a data stack, and follows the reference rules of arrays, with one exception for pools.
This chapter covers each of them,
and then iterators: any class with a `mut T next()` method, which a for-each and a repeat
loop can drive, and the library's own iterators over text and arrays.

The reasons behind the design are in [DESIGN §4.14](../DESIGN.md#414-hash-maps-hashmap-decided)
(maps), [§3.5](../DESIGN.md#35-pools-for-long-running-programs-decided) (pools) and
[§9.4](../DESIGN.md#94-statements-and-expressions-decided) (iterators and loops). The
statuses the methods fail with, and how `@` reads them, are in
[Errors and statuses](07-errors.md); the removal rule that runs through this chapter is in
[Memory and references](05-memory.md).

## Hash maps

A `hashmap<K, V>` maps keys to values. `put` adds or replaces an entry, `get` reads one,
and `has` asks whether a key is there:

```java
void main() {
    hashmap<string, i64> stock = [];
    stock.put("apple", 3);
    stock.put("pear", 5);
    stock.put("fig", 1);
    stock.put("apple", 4);
    i64 apples = stock.get("apple");
    println("${stock.len} entries, ${apples} apples");
    println("${stock.has("fig")} ${stock.has("kiwi")} ${stock.getOr("kiwi", -1)}");
    for (var e : stock) {
        println("${e.key} = ${e.value}");
    }
}
```

```output
3 entries, 4 apples
true false -1
apple = 4
pear = 5
fig = 1
```

- **`[]` is the only way to create one.** There are no map literals yet, so a map starts
  empty and is filled with `put`.
- **`len` is a property**, as on arrays: `stock.len`, without parentheses.
- **A for-each visits the entries in insertion order**, not in hash order. Replacing a
  value with `put` keeps the entry where it was.
- **A map can't be printed** with `${m}`; loop over it instead.

| Method | Result | Fails with | Notes |
|---|---|---|---|
| `put(key, value)` | `void` | | adds the entry, or replaces the value of an existing one |
| `get(key)` | `V` | `MISSING` | a copy of the value; the zero value when the key isn't there |
| `getOr(key, fallback)` | `V` | | the value, or `fallback` when the key isn't there |
| `getOrAdd(key, initial)` | `V&` | | a reference to the value, after adding `initial` if the key wasn't there |
| `has(key)` | `bool` | | |
| `remove(key)` | `void` | `MISSING` | a removal under rule R |
| `clear()` | `void` | | a removal under rule R |
| `len` | `i64` | | the number of entries |

`get`, `put`, `has`, `getOrAdd` and `remove` take constant time on average.

### Missing keys

`get` and `remove` fail with `MISSING` when the key isn't in the map. `get` then gives the
zero value of `V`. Read the status with `@` when the key may be missing, or use `getOr`:

```java
void main() {
    hashmap<string, i64> ages = [];
    ages.put("ann", 31);
    i64 bob @missing = ages.get("bob");
    println("${bob} ${status.name(missing)}");
    u8 first = @ages.remove("ann");
    u8 again = @ages.remove("ann");
    println("${first} ${status.name(again)} ${ages.len}");
}
```

```output
0 MISSING
0 MISSING 0
```

A failure nothing reads stops the program at its line, so `i64 n = m.get(k);` is right
when the key is known to be there, and stops the program when it isn't:

```java
void main() {
    hashmap<string, i64> ages = [];
    ages.put("ann", 31);
    i64 bob = ages.get("bob");
    println("${bob}");
}
```

```stderr exit=1
example.zn:4: `get` failed with MISSING, and nothing read its status
```

### Counting with `getOrAdd`

`getOrAdd(key, initial)` returns a `V&`, a reference to the value in the map, adding the
entry with `initial` first when the key is new. It is the one call to use for counting or
accumulating, since it finds the entry once:

```java
void main() {
    hashmap<string, i64> counts = [];
    for (string w : "the cat saw the dog and the cat".words()) {
        i64& c = counts.getOrAdd(w, 0);
        c++;
    }
    for (var e : counts) {
        print("${e.key}:${e.value} ");
    }
    println("");
}
```

```output
the:3 cat:2 saw:1 dog:1 and:1 
```

- **The reference survives later inserts.** Entries are appended and never move when the
  map grows, so a reference from `getOrAdd` stays valid however many entries are added
  after it.
- **`remove` and `clear` end it.** They are removals under rule R, so using the reference
  after one is a compile error (below).
- **`get` returns a copy.** To change a value in place, take a reference with `getOrAdd`
  or loop over the entries by reference.

### Looping over the entries

A for-each over a map gives each entry, with a `key` and a `value`. The entry's type has
no name, so the loop declares it with `var`, or with `var&` to change values in place:

```java
void main() {
    hashmap<string, i64> m = [];
    m.put("a", 1);
    m.put("b", 2);
    m.put("c", 3);
    @_ = m.remove("a");
    m.put("a", 10);
    m.put("b", 20);
    for (var& e : m) {
        e.value *= 2;
    }
    for (var e : m) {
        print("${e.key}=${e.value} ");
    }
    println("");
}
```

```output
b=40 c=6 a=20 
```

- **`var e` copies the entry; `var& e` refers to it.** Through a reference, `e.value`
  can be written.
- **A key that was removed and added again goes to the end**, as a new entry. `put` on a
  key that is there keeps its place (`b` above).
- **Entries added during the loop are visited too**, since the loop walks the entries in
  order and new ones are appended.
- **The key can't be changed**, even through `var&`, since the entry's place in the map
  depends on it:

```java
void main() {
    hashmap<string, i64> m = [];
    m.put("a", 1);
    for (var& e : m) {
        e.key = "x";
    }
}
```

```error
example.zn:5:11: error: a hashmap entry's key can't be changed
```

- **The loop variable's type must be the entry**, so `for (i64 v : m)` is an error:
  `expected i64, found an entry of hashmap<string, i64>`. There are no `keys()` or
  `values()` methods; read `e.key` and `e.value`.

### Keys

A key type must be `hashable` ([DESIGN §10.1](../DESIGN.md#101-built-in-bounds-decided)).
Today that means integers of every width, `bool`, enums and `string`:

```java
enum Color { RED, GREEN, BLUE }

void main() {
    hashmap<Color, string> names = [];
    names.put(Color.RED, "red");
    names.put(Color.BLUE, "blue");
    hashmap<u8, i64> bytes = [];
    bytes.put(255, 2);
    hashmap<bool, i64> flags = [];
    flags.put(true, 1);
    string blue = names.get(Color.BLUE);
    println("${blue} ${bytes.len} ${flags.len}");
}
```

```output
blue 1 1
```

- **Floats aren't hashable**, since `NaN` isn't equal to itself:

```java
void main() {
    hashmap<f64, i64> m = [];
}
```

```error
example.zn:2:5: error: a hashmap key must be hashable, and `f64` isn't
```

- **Arrays aren't keys either**, not even capped ones such as `u8[..~8]`; use a `string`
  that views the text (below).
- **A key is hashed with a random seed per run**, which protects against input made to
  collide. Iteration follows insertion order, not the hashes, so a program's output
  doesn't change from run to run.
- **Not yet**: a class whose fields are all hashable is accepted as a key by `zink check`,
  but the C backend doesn't compile one yet (``the C backend doesn't support `Pair` as a key
  yet``), nor a `Handle<T>` key. Until then, combine the fields into one integer or string
  key. See [What isn't there yet](15-not-yet.md).

### Values

Values can be any fixed-size type: numbers, strings, classes, capped and fixed arrays,
handles. They can't be anything that grows:

```java
void main() {
    hashmap<string, i64[..>]> m = [];
}
```

```error
example.zn:2:5: error: hashmap values can't grow
```

The same holds for a map of maps. A map is one growable region, and a value that could
grow inside it would have to move everything after it. Use a composite key instead, one
key that stands for the pair, or values that are indexes into one shared growable array.

A value that is a capped array or a class can be changed in place through `getOrAdd`:

```java
class Point {
    i64 x;
    i64 y;
}

void main() {
    hashmap<string, u8[..~8]> nick = [];
    nick.put("robert", "bob");
    u8[..~8]& n = nick.getOrAdd("robert", "");
    n.append("by");
    hashmap<i64, Point> at = [];
    at.put(1, new Point(2, 3));
    Point& p = at.getOrAdd(1, new Point(0, 0));
    p.x += 10;
    Point q = at.get(1);
    u8[..~8] robert = nick.get("robert");
    println("${robert} ${q.x} ${q.y}");
}
```

```output
bobby 12 3
```

**Not yet**: variable-size values, such as `u8[.]` or a class with a `u8[.*]` field, type-check
as values but the C backend doesn't compile them yet. Use a capped array or a `string`.

### String keys are views

A `string` key is a view: the map holds a pointer and a length, not the text. So the text
must outlive the map (rule S), and the compiler checks that it does. A key can view:

- a string literal, which lives for the whole run;
- a grow-only buffer (`u8[..>]`) declared before the map, which is how a program usually
  keeps its keys: read the input into one buffer, and use views into it as keys, as
  `examples/wordfreq.zn` does.

```java
void main() {
    u8[..>] text = [];
    text.append("red green red blue");
    hashmap<string, i64> counts = [];
    for (string w : text.words()) {
        i64& c = counts.getOrAdd(w, 0);
        c++;
    }
    text.append(" appending later moves nothing");
    for (var e : counts) {
        print("${e.key}:${e.value} ");
    }
    println("");
}
```

```output
red:2 green:1 blue:1 
```

The compiler rejects keys whose text could go away first. Text in an inner block is
released before the map:

```java
void main() {
    hashmap<string, i64> counts = [];
    for (i64 round = 0; round < 2; round++) {
        u8[..>] line = [];
        line.append("word${round}");
        counts.put(line, round);
    }
}
```

```error
example.zn:6:9: error: a reference into `line` can't be stored in `counts`: `line` is in an inner block, so it is released first
```

Interpolated text passed straight to `put` is a temporary that lasts only for its
statement ([Text](03-text.md)):

```java
void main() {
    hashmap<string, i64> counts = [];
    i64 n = 3;
    counts.put("n=${n}", 1);
}
```

```error
example.zn:4:5: error: this text is a temporary, built only for this statement, so `counts` can't keep a view of it
```

A buffer that can remove elements, such as a `u8[..~]`, can't hold keys at all, since a
view into a removable array can be kept only in a local variable or a parameter
([Memory and references](05-memory.md)):

```java
void main() {
    u8[..~] text = [];
    text.append("a b a");
    hashmap<string, i64> counts = [];
    for (string w : text.words()) {
        counts.put(w, 1);
    }
}
```

```error
example.zn:6:9: error: a reference into `text`, which can remove elements, can be kept only in a local variable or a parameter
```

And once views into a grow-only buffer are keys, `clear` on the buffer is a removal the
map would see:

```java
void main() {
    u8[..>] text = [];
    text.append("a b");
    hashmap<string, i64> counts = [];
    for (string w : text.words()) {
        counts.put(w, 1);
    }
    text.clear();
    println("${counts.len}");
}
```

```error
example.zn:8:5: error: `clear` removes from `text` while `counts` may still refer into it
```

A map in a static lives for the whole run, so its string keys must too: literals, or
text in another static.

### References into a map, and removals

A map follows the rules for removable arrays ([Memory and references](05-memory.md),
[REFERENCES §9](../REFERENCES.md#9-other-cases)):

- **Inserts invalidate nothing.** `put` and `getOrAdd` never move an entry, so references
  from `getOrAdd` and from a for-each by reference survive them.
- **`remove` and `clear` are removals under rule R.** A reference into the map can't be
  used after one:

```java
void main() {
    hashmap<string, i64> m = [];
    i64& c = m.getOrAdd("a", 0);
    @_ = m.remove("a");
    c++;
}
```

```error
example.zn:4:10: error: `remove` removes from `m` while `c` may still refer into it
```

- **No removal inside a for-each over the map**, by value or by reference, since the loop
  keeps its place in the entries:

```java
void main() {
    hashmap<string, i64> m = [];
    m.put("a", 1);
    for (var e : m) {
        if (e.value == 1) {
            @_ = m.remove(e.key);
        }
    }
}
```

```error
example.zn:6:18: error: `remove` removes from `m` inside a for-each over it
```

  To remove some entries, collect their keys first and remove them after the loop. A key
  is a `string` viewing the key's own text, not the map, so the list of keys can outlive
  the removals:

```java
void main() {
    hashmap<string, i64> stock = [];
    stock.put("bolt", 0);
    stock.put("nut", 12);
    stock.put("gear", 0);
    stock.put("cog", 3);
    string[..>] empty = [];
    for (var e : stock) {
        if (e.value == 0) {
            empty +> e.key;
        }
    }
    for (string k : empty) {
        stock.remove(k);
    }
    for (var e : stock) {
        print("${e.key}=${e.value} ");
    }
    println("");
}
```

```output
nut=12 cog=3 
```

- **References into a map can be held only in local variables and parameters**, never in
  fields, array elements or statics, so the compiler can see every one that might still
  be used when something is removed:

```java
class Holder {
    i64& r;
}

void main() {
    hashmap<string, i64> m = [];
    i64& c = m.getOrAdd("a", 0);
    Holder h = new Holder(c);
}
```

```error
example.zn:8:16: error: a reference into `m`, which can remove elements, can be kept only in a local variable or a parameter
```

  A structure whose entries point at each other belongs in a `pool` (below), or stores
  keys or indexes instead of references.

### Maps in classes, statics and parameters

A map is one growable region, so a class can hold one as its growable field. It starts
empty, so the default constructor doesn't take it:

```java
class Index {
    string name;
    hashmap<string, i64> seen;

    mut i64 note(string w) {
        i64& c = seen.getOrAdd(w, 0);
        c++;
        return c;
    }
}

void main() {
    Index ix = new Index("words");
    i64 a = ix.note("x");
    i64 b = ix.note("x");
    i64 c = ix.note("y");
    println("${ix.name}: ${a} ${b} ${c}, ${ix.seen.len} distinct");
}
```

```output
words: 1 2 1, 2 distinct
```

- **A class with two maps, or a map and an array that grows, is a root**
  ([Classes, interfaces and generics](06-classes.md),
  [DESIGN §6.6](../DESIGN.md#66-roots-several-growable-fields-decided)), which gives each
  growable field a data stack of its own. A plain class with two is an error that
  suggests it: ``a class can hold only one growable field, and `a` is already one``.
- **A static map** (`hashmap<string, i64> PRICES = [];` at the top level) works like a
  local one, for the whole run.
- **A function that changes a map takes it by reference**, `hashmap<string, i64>& m`. A
  plain parameter is a read-only copy, so calling `put` on one is an error:
  ``can't call `put` here, since it changes what it's called on: `counts` is a plain
  parameter, which is a read-only copy``.

```java
void tally(hashmap<string, i64>& counts, string word) {
    i64& c = counts.getOrAdd(word, 0);
    c++;
}

i64 total(hashmap<string, i64> counts) {
    i64 t = 0;
    for (var e : counts) {
        t += e.value;
    }
    return t;
}

void main() {
    hashmap<string, i64> counts = [];
    for (string w : "a b a c a".words()) {
        tally(counts, w);
    }
    println("${total(counts)} words, ${counts.len} distinct");
}
```

```output
5 words, 3 distinct
```

**Not yet**: copying a map (`hashmap<string, i64> b = a;`) and returning one from a
function both type-check, but the C backend doesn't compile them yet (`the C backend
doesn't support copying a map, a set or a pool yet`). Pass a map by reference instead.

### How a map is stored

A map follows Python's compact dict, in one region of a data stack: the entries in
insertion order, each with its key, its value and, for a string key, the key's hash; and
an index of 1-, 2-, 4- or 8-byte slots, as narrow as the capacity allows, that finds an
entry by linear probing. Growing rebuilds only the index, so entries never move. Removing
marks an entry dead, which keeps the order, and the entries are compacted once more than
half of them are dead. DESIGN §4.14 and §15 compare it with the alternatives that were
measured.

## Hash sets

A `hashset<T>` is a map without values: the same layout, key rules and reference rules.
`add` returns `true` when the element wasn't there yet, which makes "first time seen" one
call:

```java
void main() {
    hashset<string> seen = [];
    i64 fresh = 0;
    for (string w : "b a b c a b".words()) {
        if (seen.add(w)) {
            fresh++;
        }
    }
    println("${fresh} ${seen.len} ${seen.has("c")} ${seen.has("d")}");
    for (string w : seen) {
        print("${w} ");
    }
    println("");
    seen.remove("a");
    u8 again = @seen.remove("a");
    println("${status.name(again)} ${seen.len}");
    seen.add("a");
    for (string w : seen) {
        print("${w} ");
    }
    println("");
}
```

```output
3 3 true false
b a c 
MISSING 2
b c a 
```

| Method | Result | Fails with | Notes |
|---|---|---|---|
| `add(x)` | `bool` | | `true` when `x` wasn't there and was added |
| `has(x)` | `bool` | | there is no `contains` on a set |
| `remove(x)` | `void` | `MISSING` | a removal under rule R |
| `clear()` | `void` | | a removal under rule R |
| `len` | `i64` | | |

- **A for-each visits the elements in insertion order**, by value only. An element is a
  key, which can't be changed in place, so `for (i64& x : s)` is an error: ``these values
  can't be referred to with `&` ``.
- **Elements follow the key rules of maps**: they must be `hashable`, and a `string`
  element is a view whose text must outlive the set. `seen.add("n${n}")` is rejected as
  `put` is above.
- **No removal inside a for-each over the set**, as for maps.

Generic code that builds a set of its type parameter needs the `hashable` bound:

```java
<K implements hashable> i64 distinct(K[:] xs) {
    hashset<K> seen = [];
    for (K x : xs) {
        seen.add(x);
    }
    return seen.len;
}

void main() {
    i64[..>] nums = [3, 1, 3, 2, 1];
    string[..>] words = ["a", "b", "a"];
    println("${distinct(nums)} ${distinct(words)}");
}
```

```output
3 2
```

Without the bound, the set is an error:

```java
<K> i64 distinct(K[:] xs) {
    hashset<K> seen = [];
    return seen.len;
}

void main() {
    i64[..>] nums = [3, 1];
    println("${distinct(nums)}");
}
```

```error
example.zn:2:5: error: a hashset element must be hashable, and `K` isn't
```

## Pools and handles

An array gives its memory back only when its scope ends. A program that keeps adding and
dropping objects for its whole run, such as a cache or a server's connections, would then
grow without end. A `pool<T>` is the answer: a grow-only array of slots with a free list,
where `add` reuses a freed slot before it grows
([DESIGN §3.5](../DESIGN.md#35-pools-for-long-running-programs-decided)).

`add` returns a `Handle<T>`: the slot's index and a generation number, 8 bytes in all.
`get` turns a handle back into a reference, and `free` returns the slot:

```java
class Conn {
    i64 fd;
    string name;
}

void main() {
    pool<Conn> conns = [];
    Handle<Conn> a = conns.add(new Conn(3, "a"));
    Handle<Conn> b = conns.add(new Conn(4, "b"));
    Handle<Conn> c = conns.add(new Conn(5, "c"));
    Conn& cb = conns.get(b);
    cb.fd = 40;
    println("${conns.len} live, b is fd ${cb.fd}");
    conns.free(b);
    u8 twice = @conns.free(b);
    Conn& gone @stale = conns.get(b);
    println("free again: ${status.name(twice)}, get: ${status.name(stale)}");
    Handle<Conn> d = conns.add(new Conn(6, "d"));
    println("d reuses b's slot: ${b == d}, ${d == d}");
    for (Conn& x : conns) {
        print("${x.fd}:${x.name} ");
    }
    println("");
}
```

```output
3 live, b is fd 40
free again: MISSING, get: MISSING
d reuses b's slot: false, true
3:a 6:d 5:c 
```

| Method | Result | Fails with | Notes |
|---|---|---|---|
| `add(x)` | `Handle<T>` | | stores `x` in the slot freed last, or in a new slot at the end |
| `get(h)` | `T&` | `MISSING`, `INVALID` | a reference to the slot's value |
| `free(h)` | `void` | `MISSING`, `INVALID` | marks the slot free; its value stays until the slot is reused |
| `clear()` | `void` | | frees every slot, so every handle fails |
| `len` | `i64` | | the number of live slots |

- **Freeing bumps the slot's generation**, so a handle from before the free no longer
  matches: `get` and `free` fail with `MISSING`, even after the slot is reused. Two
  handles are equal with `==` when they name the same slot and generation, which is why
  `b == d` is false above.
- **A handle's type names the pool's element type**, so a `Handle<Conn>` passed to a
  `pool<Job>` is a compile error: `expected Handle<Job>, found Handle<Conn>`.
- **A for-each visits the live slots in slot order**, by value or by reference.
- **A handle can't be printed**, and isn't a map key yet; keep it as a map value, as
  `examples/cachesim.zn` does with `hashmap<K, Handle<Node<K>>>`.

A failed `get` still returns a reference: to the slot's value for a stale handle, which a
pool slot always holds, and otherwise to a zero value of its own. So nothing can read
outside the pool, but code must read the status before it trusts the value. A `get` whose
failure nothing reads stops the program:

```java
void main() {
    pool<i64> nums = [];
    Handle<i64> h = nums.add(42);
    nums.free(h);
    i64& n = nums.get(h);
    println("${n}");
}
```

```stderr exit=1
example.zn:5: `get` failed with MISSING, and nothing read its status
```

### Handles from somewhere else

A handle carries no pointer to its pool, so the pool can check only its index and
generation:

- **A slot index the pool never had fails with `INVALID`**: a handle from a larger pool
  of the same type, say. `get` then gives a reference to a fresh zero value.
- **The zero value of a `Handle<T>`**, as a failed call gives (a `get` of a missing key
  in a `hashmap<string, Handle<T>>`, say), names slot 0 with generation 0, which no live
  slot has. It fails with `MISSING` when the pool has a slot 0, and with `INVALID` when it
  is empty.
- **A handle from another pool whose slot exists in this one** usually fails with
  `MISSING`, but when the generations happen to match it reads this pool's slot. That is a
  logic bug, never memory corruption: the slot always holds a value of the right type.

```java
class Conn {
    i64 fd;
}

void main() {
    pool<Conn> small = [];
    pool<Conn> big = [];
    Handle<Conn> b1 = big.add(new Conn(1));
    Handle<Conn> b2 = big.add(new Conn(2));
    Handle<Conn> s1 = small.add(new Conn(9));
    Conn& x @beyond = small.get(b2);
    println("slot 1 of a 1-slot pool: ${status.name(beyond)}, reads ${x.fd}");
    Conn& y @wrong = big.get(s1);
    println("slot 0 of the other pool: ${wrong}, reads ${y.fd}");
    hashmap<string, Handle<Conn>> byName = [];
    Handle<Conn> none @missing = byName.get("nobody");
    Conn& z @zero = small.get(none);
    println("a zero handle: ${status.name(zero)}");
}
```

```output
slot 1 of a 1-slot pool: INVALID, reads 0
slot 0 of the other pool: 0, reads 1
a zero handle: MISSING
```

### References into a pool

A reference into a pool is exempt from rule R
([REFERENCES §5.4](../REFERENCES.md#54-capped-arrays-and-pools)): `free` doesn't end it,
and it may be kept in a field, an element or a static, as rule S allows. That's safe
because a slot's memory is never released or given another type while the pool lives. A
stale reference reads the freed value until the slot is reused, and then the value that
replaced it: a logic bug, never memory corruption.

So objects in a pool can point at each other directly, which objects in a removable array
can't. Here each task links to the next through a reference field:

```java
class Task {
    string name;
    Task&? next;
}

void main() {
    pool<Task> tasks = [];
    Handle<Task> ha = tasks.add(new Task("write", null));
    Handle<Task> hb = tasks.add(new Task("test", null));
    Task& a = tasks.get(ha);
    Task& b = tasks.get(hb);
    a.next .= b;
    tasks.free(hb);
    println("b after its free: ${b.name}");
    Handle<Task> hc = tasks.add(new Task("ship", null));
    Task&? n = a.next;
    if (n != null) {
        println("a.next now reads ${n.name}");
    }
    Task& old @gone = tasks.get(hb);
    println("the old handle: ${status.name(gone)}");
}
```

```output
b after its free: test
a.next now reads ship
the old handle: MISSING
```

The handle catches what the reference can't: keep handles where a slot may be freed
behind the code's back, and references for links that the code unlinks itself before it
frees, as `examples/cachesim.zn`'s LRU list does.

### Changing a pool during a loop

`add` and `free` are allowed inside a for-each over the pool, since a pool isn't under
rule R. The loop walks the slots the pool had when it started, in order, and visits each
one that is live when it gets there:

```java
class Job {
    i64 id;
}

void main() {
    pool<Job> jobs = [];
    Handle<Job> a = jobs.add(new Job(1));
    Handle<Job> b = jobs.add(new Job(2));
    Handle<Job> c = jobs.add(new Job(3));
    for (Job& j : jobs) {
        if (j.id == 1) {
            jobs.free(b);
            jobs.add(new Job(4));
            jobs.add(new Job(5));
        }
        print("${j.id} ");
    }
    println("len ${jobs.len}");
    for (Job j : jobs) {
        print("${j.id} ");
    }
    println("");
}
```

```output
1 4 3 len 4
1 4 3 5 
```

- A slot freed before the loop reaches it is skipped.
- A slot freed and then reused before the loop reaches it is visited with its new value
  (`4` above).
- Slots added past the end during the loop aren't visited (`5`).

### Freed slots are reused last-freed first

`add` takes the slot freed most recently, and grows only when none is free. `clear` frees
every slot, and the slots are then reused from the first:

```java
void main() {
    pool<string> names = [];
    Handle<string> a = names.add("ann");
    Handle<string> b = names.add("bo");
    Handle<string> c = names.add("cy");
    names.free(a);
    names.free(c);
    Handle<string> d = names.add("di");
    for (string n : names) {
        print("${n} ");
    }
    println("len ${names.len}");
    names.clear();
    Handle<string> e = names.add("ed");
    Handle<string> f = names.add("flo");
    for (string n : names) {
        print("${n} ");
    }
    string first = names.get(e);
    println("len ${names.len}, first ${first}");
}
```

```output
bo di len 2
ed flo len 2, first ed
```

- **A pool of strings is read by copying**: `string s = names.get(h);`. The reference
  `get` returns can't be bound as `string&`, which is a writable view (`u8[:]&`,
  [Arrays](04-arrays.md)), not a reference to a string, so `string& s = names.get(h);` is
  an error: `a read-only slice can't become a writable view`.
- **The memory stays.** A pool's slots are never given back while it lives, so a pool
  is as large as the most values it held at once.
- **Not yet**: a pool of variable-size values (a class with a `u8[.*]` field, say)
  type-checks but doesn't compile yet; copying or returning a pool doesn't either. See
  [What isn't there yet](15-not-yet.md).

## Iterators

An iterator is any value whose class has a `mut T next()` method that gives the next
value, and fails with a status, by convention `END`, when there are no more
([DESIGN §9.4](../DESIGN.md#94-statements-and-expressions-decided)). There is nothing to
inherit: a for-each needs only the method.

```java
class Countdown {
    i64 n;

    mut i64 next() {
        if (n == 0) {
            return #status.END 0;
        }
        n--;
        return n + 1;
    }
}

void main() {
    for (i64 x : new Countdown(3)) {
        print("${x} ");
    }
    println("");
}
```

```output
3 2 1 
```

- **`next()` is `mut`**, since it moves the iterator along.
- **At the end it returns the zero value with `#status.END`** ([Errors and
  statuses](07-errors.md)). Any nonzero status ends a loop; `END` says it ended normally.
- **A for-each over an iterator is a repeat loop on a hidden local**: `for (T x : e)`
  means `var it = e;` and then `while (T x : it.next())`. So `break`, `continue` and
  `return` work as in any loop, and no array is built.
- **The class is known at compile time**, so `next()` is a direct call the C compiler can
  inline, and the loop costs what a hand-written one would.

### The repeat loop

To steer an iterator from inside the loop, or to look at it afterwards, keep it in a
variable and call `next()` in a repeat loop, `while (T x : it.next())`. The call is made
before every pass; while its status is 0 its value is bound to `x` and the body runs. A
status capture after the header, `@why`, declares a variable in the enclosing block that
holds the status that ended the loop, or 0 when a `break` did:

```java
class Countdown {
    i64 n;

    mut i64 next() {
        if (n == 0) {
            return #status.END 0;
        }
        n--;
        return n + 1;
    }
}

void main() {
    Countdown c = new Countdown(5);
    i64 first = c.next();
    while (i64 x : c.next()) @why {
        if (x == 2) {
            break;
        }
        print("${x} ");
    }
    println("first ${first}, ${c.n} left, why ${why}");
    while (i64 x : c.next()) @why2 {
        print("${x} ");
    }
    println("why2 ${status.name(why2)}");
}
```

```output
4 3 first 5, 1 left, why 0
1 why2 END
```

The `break` came after `next()` had returned 2, so the second loop picks up at 1. A
`break` leaves without another call, and `continue` makes the next one.

The capture matters when an iterator can fail for a reason other than the end. Here
`next()` passes on the failure of `i64.parse` with `return ##`, and the caller tells the
two apart:

```java
class Numbers implements Iterator<i64> {
    WordIterator words;

    mut i64 next() {
        string w @end = words.next();
        if (end) {
            return #end 0;
        }
        return ## i64.parse(w);
    }
}

void sum(string text) {
    Numbers nums = new Numbers(text.words());
    i64 total = 0;
    while (i64 n : nums.next()) @why {
        total += n;
    }
    if (why != status.END) {
        println("stopped by ${status.name(why)} after ${total}");
        return;
    }
    println("total ${total}");
}

void main() {
    sum("1 2 3");
    sum("4 five 6");
}
```

```output
total 6
stopped by INVALID after 4
```

- **`for` evaluates its source once; `while` calls again on every pass**, so the call is
  written with its parentheses.
- **The source is anything `@` can read**: a call, or an array operator such as
  `todo ->` ([Arrays](04-arrays.md)). Library readers that take a buffer are looped over
  the same way: `while (string line : stdin.nextLine(buf))`
  ([The library](13-library-system.md)).
- **With a capture, the body must be a block.** A for-each has no capture; use a repeat
  loop when the reason matters.
- **The header can destructure a tuple**, for an iterator whose `next()` returns several
  values. A for-each can't, so this needs the repeat loop:

```java
class Settings {
    LineIterator lines;

    mut (string, string) next() {
        string line @end = lines.next();
        if (end) {
            return #end ("", "");
        }
        return line.splitFirst("=");
    }
}

void main() {
    Settings s = new Settings("a=1\nb=2\nc=3".lines());
    while ((string key, string value) : s.next()) {
        print("${key}->${value} ");
    }
    println("");
}
```

```output
a->1 b->2 c->3 
```

Three mistakes the compiler catches. A for-each over an iterator variable would change
a variable the loop doesn't name, so it's an error that suggests the repeat loop:

```java
void main() {
    LineIterator lines = "a\nb".lines();
    for (string l : lines) {
        println(l);
    }
}
```

```error
example.zn:3:21: error: a for-each over an iterator variable isn't allowed
```

A repeat loop over a call that never sets a status would never end:

```java
i64 roll() {
    return 4;
}

void main() {
    while (i64 x : roll()) {
        println("${x}");
    }
}
```

```error
example.zn:6:20: error: this never sets a status, so the loop would never end
```

That includes a `next()` that calls another iterator and doesn't pass its failure on: a
failure nothing reads inside it stops the program rather than ending the loop, so the
method never fails, and the loop is rejected the same way.

A class with neither `next()` nor `iterator()` can't be looped over:
`` `Box` has no `next()` method, so a for-each can't loop over it ``.

### The library's iterators

The library's lazy walks over text and arrays are iterator classes in `zink.lang`, each
implementing `Iterator<T>`. Each part they give is a view into what they walk, so nothing
is copied:

| Call | Class | Gives | Details |
|---|---|---|---|
| `s.lines()` | `LineIterator` | `string` | lines without their `\n` or `\r\n`; no empty last line ([Text](03-text.md)) |
| `s.words()` | `WordIterator` | `string` | runs of non-whitespace ([Text](03-text.md)) |
| `s.splitBy(sep)` | `SplitIterator` | `string` | the parts between separators, empty ones included ([Text](03-text.md)) |
| `s.codePoints()` | `CodePointIterator` | `u32` | decoded UTF-8, U+FFFD for invalid bytes ([Text](03-text.md)) |
| `a.chunks(n)` | `ChunkIterator<T>` | `T[:]` | slices of `n` elements, the last one shorter ([Arrays](04-arrays.md)) |
| `a.windows(n)` | `WindowIterator<T>` | `T[:]` | every run of `n` consecutive elements ([Arrays](04-arrays.md)) |

```java
void main() {
    string text = "name=ann\nrole=dev\n";
    for (string line : text.lines()) {
        (string k, string v) = line.splitFirst("=");
        print("${k}:${v} ");
    }
    println("");
    for (string p : "a;b;;c".splitBy(";")) {
        print("[${p}]");
    }
    println("");
    for (u32 c : "aé€".codePoints()) {
        print("${c} ");
    }
    println("");
    i64[..>] xs = [1, 2, 3, 4, 5];
    for (i64[:] c : xs.chunks(2)) {
        print("${c} ");
    }
    println("");
    for (i64[:] w : xs.windows(3)) {
        print("${w} ");
    }
    println("");
}
```

```output
name:ann role:dev 
[a][b][][c]
97 233 8364 
[1, 2] [3, 4] [5] 
[1, 2, 3] [2, 3, 4] [3, 4, 5] 
```

They can be kept in variables of their class and stepped by hand. `chunks` and `windows`
fail with `INVALID` for a size below 1, and the iterator then gives nothing:

```java
void main() {
    i64[..>] xs = [1, 2, 3, 4, 5];
    ChunkIterator<i64> ch = xs.chunks(4);
    i64[:] head = ch.next();
    i64[:] rest = ch.next();
    println("${head} ${rest}");
    ChunkIterator<i64> bad @e = xs.chunks(0);
    i64 n = 0;
    while (i64[:] c : bad.next()) {
        n++;
    }
    println("${status.name(e)}, ${n} chunks");
    LineIterator lines = "first\nsecond\nthird".lines();
    string skip = lines.next();
    while (string l : lines.next()) {
        print("<${l}>");
    }
    println("");
}
```

```output
[1, 2, 3, 4] [5]
INVALID, 0 chunks
<second><third>
```

In a for-each, an `INVALID` from `chunks(0)` that nothing reads stops the program, as
any failure does: `` `chunks` failed with INVALID, and nothing read its status ``.

### A class that gives an iterator

A class with an `iterator()` method, which returns an iterator, can be looped over
directly, like Java's `Iterable`. The for-each calls `iterator()` once:

```java
class Steps {
    i64 at;
    i64 end;
    i64 by;

    mut i64 next() {
        if (at >= end) {
            return #status.END 0;
        }
        at += by;
        return at - by;
    }
}

class Range {
    i64 lo;
    i64 hi;

    Steps iterator() {
        return new Steps(lo, hi, 1);
    }

    Steps by(i64 step) {
        return new Steps(lo, hi, step);
    }
}

void main() {
    Range r = new Range(2, 7);
    for (i64 i : r) {
        print("${i} ");
    }
    println("");
    for (i64 i : r.by(2)) {
        print("${i} ");
    }
    println("");
}
```

```output
2 3 4 5 6 
2 4 6 
```

`r` is a variable, but not an iterator: `Range` has no `next()`, and looping over it
changes nothing in it. Other ways to walk it, such as `by(2)`, are methods that return
an iterator.

### Iterators that give references

A for-each by reference, `for (T& x : e)`, needs a `next()` that returns a reference,
`mut T& next()`. With a plain `mut T next()`, `for (i64& x : ...)` is an error: ``these
values can't be referred to with `&` ``. A reference iterator still has to return a
reference at the end, so it keeps one to return with `END`: here a static that is never
written.

```java
class Item {
    string name;
    i64 qty;
}

Item NONE = new Item("", 0);

class LowStock {
    Item[..>]& items;
    i64 at = 0;

    mut Item& next() {
        while (at < items.len) {
            at++;
            if (items[at - 1].qty < 5) {
                return items[at - 1];
            }
        }
        return #status.END NONE;
    }
}

void main() {
    Item[..>] stock = [];
    stock +> new Item("bolt", 2) +> new Item("nut", 40) +> new Item("gear", 1);
    for (Item& it : new LowStock(stock)) {
        it.qty += 10;
    }
    for (Item it : stock) {
        print("${it.name}:${it.qty} ");
    }
    println("");
}
```

```output
bolt:12 nut:40 gear:11 
```

### `Iterator<T>` and generic code

`zink.lang` declares the interface every library iterator implements
([STDLIB §2.5](../STDLIB.md#25-iterators)):

```java
public interface Iterator<T> {
    mut T next();
}
```

A for-each doesn't need it. It is for code that takes iterators of different classes:

- **`Iterator<T>&`**, a reference through the interface, calls `next()` through a method
  table ([Classes, interfaces and generics](06-classes.md));
- **`<I implements Iterator<T>>`**, a generic bound, compiles a copy per class, with
  `next()` called directly.

```java
class Repeat implements Iterator<string> {
    string word;
    i64 times;

    mut string next() {
        if (times == 0) {
            return #status.END "";
        }
        times--;
        return word;
    }
}

class Countdown implements Iterator<i64> {
    i64 n;

    mut i64 next() {
        if (n == 0) {
            return #status.END 0;
        }
        n--;
        return n + 1;
    }
}

i64 longest(Iterator<string>& it) {
    i64 best = 0;
    while (string s : it.next()) {
        best = max(best, s.len);
    }
    return best;
}

<I implements Iterator<i64>> i64 total(I& it) {
    i64 sum = 0;
    while (i64 x : it.next()) {
        sum += x;
    }
    return sum;
}

void main() {
    LineIterator lines = "a\nbbb\ncc".lines();
    WordIterator words = "one three five".words();
    Repeat r = new Repeat("hey", 2);
    println("${longest(lines)} ${longest(words)} ${longest(r)}");
    Countdown c = new Countdown(4);
    println("${total(c)}, and ${c.n} left in c");
}
```

```output
3 5 3
10, and 0 left in c
```

- **The parameter is a reference**, since `next()` changes the iterator, so the caller's
  iterator is used up (`c.n` is 0 afterwards).
- **So it needs a variable**: `longest("a b".words())` is an error, `a reference needs a
  variable, field or element to refer to, and this is a temporary value`.
- **Inside, it's an iterator variable**, so the function loops with
  `while (T x : it.next())`; `for (string s : it)` is the error above.

### Iterators and the reference rules

An iterator over text or an array holds a view into it, so the rules for views apply
([Memory and references](05-memory.md), [REFERENCES §9](../REFERENCES.md#9-other-cases)):

- **A for-each keeps its iterator for the whole loop**, so removing from what it walks
  inside the loop is an error: `` `clear` removes from `text` inside a for-each over it ``.
- **An iterator in a variable keeps its view as long as it's used**:

```java
void main() {
    u8[..~] text = [];
    text.append("a\nb\n");
    LineIterator it = text.lines();
    text.clear();
    while (string l : it.next()) {
        println(l);
    }
}
```

```error
example.zn:5:5: error: `clear` removes from `text` while `it` may still refer into it
```

- **An iterator over a temporary can't be used**, since the temporary is released at the
  end of its statement. That holds for a for-each too, whose iterator is a hidden local
  that outlives the header. Keep the text in a variable first:

```java
u8[.] make() {
    u8[..>] t = [];
    t.append("a\nb");
    return t;
}

void main() {
    for (string l : make().lines()) {
        print("<${l}>");
    }
}
```

```error
example.zn:8:21: error: this refers into a temporary value, which is released at the end of the statement
```

  With `u8[.] text = make();` first, `for (string l : text.lines())` works.
- **A repeat loop's variables are declared afresh on each pass**, after the call, so the
  call may remove from what the previous pass's value viewed: `stdin.nextLine(buf)`
  clears `buf` before reading each line.
