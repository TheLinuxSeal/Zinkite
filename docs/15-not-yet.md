# 15. What isn't there yet

This chapter lists what the design describes and the compiler doesn't do yet, what the
design hasn't decided, and the limits a program meets today, by area: the language,
programs and files, arrays and text, classes, statuses, memory, calling C, threads, the
library and the tools. Each item says what it would be and what to write today instead.
Everything here was checked against the compiler as of November 2026; the other chapters
mention the same items where they come up.

Each item is one of three kinds:

- **Undecided**: an open decision in [DESIGN §17](../DESIGN.md#17-open-decisions),
  [GRAMMAR §8](../GRAMMAR.md#8-not-in-v2-open) or [REFERENCES
  §12](../REFERENCES.md#12-open-questions). Until it is decided, the compiler rejects
  programs that use it, and most such errors say `undecided`. Deciding one later only makes
  more programs valid, so nothing that compiles today breaks.
- **Not built**: decided and designed, and not in the compiler yet. Often `zink check`
  accepts the program and code generation refuses it; sometimes the compiler doesn't check
  a rule yet, and accepts what the design rejects.
- **Limit**: how things are today, with no decision pending. Some may change; none is a
  bug.

A last section lists what the compiler accepts today and compiles wrongly, and one after it
what older notes call missing that is there now.

## How the compiler says so

An undecided feature is an error that names the feature:

```java
class Node {
    i64 value;
}

void main() {
    Node n = new Node(1);
    Node&? maybe = n;
    println("${maybe?.value}");
}
```

```error
example.zn:8:22: error: null-safe access `?.` is undecided (GRAMMAR §8)
```

A feature the type checker knows and the C backend doesn't generate yet passes `zink
check`, and `zink run`, `zink build` and `zink c` refuse it, all with the same help line:

```java
class Point {
    i64 x;
    i64 y;
}

void main() {
    Point a = new Point(1, 2);
    Point b = new Point(1, 2);
    println("${a == b}");
}
```

```error
example.zn:9:16: error: the C backend doesn't support comparing class values yet
help: `zink check` accepts it; code generation for it comes in a later milestone
```

A `zink.*` package that isn't written yet is a warning when it is imported, and a program
that uses a name from it doesn't compile to C ([Programs, packages and
building](10-programs.md#imports)).

## The language

### Syntax that is undecided

Everything in this section is undecided, in [GRAMMAR §8](../GRAMMAR.md#8-not-in-v2-open)
or DESIGN §17. Most are errors that say `undecided`; the rest are syntax errors or unknown
names today, as each item says.

- **Hash map literals with entries**, `["http": 80]`, and indexing a map with
  `m[key]`. Start with `[]` and `put`, and read with `get`, `getOr` or `getOrAdd`
  ([Maps, sets, pools and iterators](09-collections.md)):

```java
void main() {
    hashmap<string, i64> ports = ["http": 80, "https": 443];
    println("${ports.len}");
}
```

```error
example.zn:2:41: error: hash map literals with entries are undecided (GRAMMAR §8); start with `[]` and `put`
```

```java
void main() {
    hashmap<string, i64> ports = [];
    ports.put("http", 80);
    ports.put("https", 443);
    i64 p @_ = ports.get("https");
    println("${ports.len} ${p}");
}
```

```output
2 443
```

- **Null-safe access `?.`**, shown above, and a null-coalescing operator such
  as `??` (a syntax error). Test for `null`, which narrows the reference after it:

```java
class Node {
    i64 value;
}

i64 valueOr(Node&? n, i64 fallback) {
    if (n == null) {
        return fallback;
    }
    return n.value;
}

void main() {
    Node n = new Node(7);
    Node&? maybe = n;
    Node&? none = null;
    println("${valueOr(maybe, 0)} ${valueOr(none, 0)}");
}
```

```output
7 0
```

- **Multi-dimensional indexing** `m[i, j]`, and multi-dimensional arrays. Keep
  one array and index it as `y * width + x`, or nest fixed arrays, `i64[.3][.2]`:

```java
void main() {
    i64[.] grid = [1, 2, 3, 4, 5, 6];
    println("${grid[1, 2]}");
}
```

```error
example.zn:3:22: error: multi-dimensional indexing `m[i, j]` is undecided (GRAMMAR §8)
```

- **A clamped slice bound** `xs[:|n]`, to match the clamped index `xs[|i]`.
  Write `xs[:min(n, xs.len)]`.
- **Default and static methods in interfaces**. A method body in an interface is an error,
  `interface methods have no bodies: default methods are undecided (GRAMMAR §8)`, and a
  `static` method in one is `static methods in interfaces are undecided`. Write a static
  function that takes the interface reference.
- **Nested type declarations**, a class inside a class. Declare it at the top
  level, package-private.
- **`final` fields and locals** (`final` is reserved). There are no constants
  either (below, "Limits").
- **Enums with fields, methods or payloads**, and **patterns in `switch`**
  (a syntax error today). An enum is a list of constants; keep data about each in a
  `switch` that returns it, or in a table indexed by a number you keep beside it.
  Variants with payloads would close the largest gap left between Zinkite and Goose, as
  [DESIGN §15](../DESIGN.md#15-performance) measures.
- **Static initializer blocks**, `static { ... }`. Give each static an
  initializer that calls a function ([Programs, packages and
  building](10-programs.md#static-initialization)).
- **`#elif`**. Nest an `#if` inside the `#else`
  ([Programs, packages and building](10-programs.md#conditional-compilation)).
- **Unicode letters in identifiers**. Identifiers are ASCII; text and
  comments may hold any UTF-8.
- **Method references** such as `this::close`, and a function's name used as a value
  (DESIGN §9.3). Wrap the call in a lambda:

```java
void report() {
    println("done");
}

void main() {
    func(void => void) f = report;
    f();
}
```

```error
example.zn:6:28: error: `report` is a function; call it with `()`
help: method references are undecided (DESIGN §9.3)
```

```java
void report() {
    println("done");
}

void main() {
    func(void => void) f = () => report();
    f();
}
```

```output
done
```

- **Explicit type arguments on a generic call**, `first<i64>(xs)`. Today the call parses as
  comparisons, and the error is `` `i64` is a type, not a value ``. Type arguments come
  from the arguments ([Classes, interfaces and generics](06-classes.md)); pass a typed local
  where they can't be inferred.
- **Compile-time constant type parameters**, `<const N: i32>`, and so caps chosen by a
  generic class (a syntax error today). Write the cap as a number in each class.
- **Varargs** (`i64... xs`, a syntax error). Take a slice, `i64[:] xs`, and pass an array
  literal: `sum([1, 2, 3])`.
- **Multi-line strings** (`"""`, a syntax error: strings can't span lines). Write `\n`, or
  append the lines to a buffer.
- **`assert`** (`assert(x)` is an unknown function). Whether it would be a statement or a
  library function, with `zink.test`, is open (STDLIB §10.5). Test the condition and fail
  with a status, as `if (!ok) { return #status.INVALID; }` in a `void` function, which
  stops the program if nothing reads it.
- **Type aliases**, `type Action = func(void => void);` (a syntax error). Write the type out.
- **Annotations** such as `@Override` (a syntax error). `@` reads statuses instead.
- **Integers narrower than a byte**, `i4` and `u4` (`unknown type`; DESIGN §9.1). Pack them into a `u8`
  with shifts and masks.

### Values and types

- **Tuples as values** (undecided): a tuple in a variable, a field or an array, and `t.0`.
  A tuple is only a result, destructured where it is returned ([Functions and
  lambdas](08-functions.md)). Destructure it at once, and keep the parts in a class if they
  must travel together:

```java
(i64, i64) minMax(i64[:] xs) {
    i64 lo = xs[0];
    i64 hi = xs[0];
    for (i64 x : xs) {
        lo = min(lo, x);
        hi = max(hi, x);
    }
    return (lo, hi);
}

void main() {
    i64[.] xs = [4, 1, 9];
    (i64, i64) both = minMax(xs);
    println("${both.0}");
}
```

```error
example.zn:13:6: error: `i64` is a type, not a value
example.zn:14:21: error: expected a member name after `.`, found an integer
```

```java
(i64, i64) minMax(i64[:] xs) {
    i64 lo = xs[0];
    i64 hi = xs[0];
    for (i64 x : xs) {
        lo = min(lo, x);
        hi = max(hi, x);
    }
    return (lo, hi);
}

class Range {
    i64 lo;
    i64 hi;
}

void main() {
    i64[.] xs = [4, 1, 9];
    (i64 lo, i64 hi) = minMax(xs);
    Range kept = new Range(lo, hi);
    println("${kept.lo} ${kept.hi}");
}
```

```output
1 9
```

- **Printing a class value or an enum** with `${x}` (undecided, [DESIGN
  §4.13](../DESIGN.md#413-printing-and-formatting-decided)). Print the fields, and give an
  enum a function that names its constants:

```java
enum Color { RED, GREEN, BLUE }

void main() {
    Color c = Color.GREEN;
    println("${c}");
}
```

```error
example.zn:5:16: error: an enum value can't be printed until default formatting is decided (DESIGN §17)
```

- **No conversion between an enum and an integer** (limit). A cast is `` casts exist only
  for numbers ``, so an enum can't index an array or be written to a file as a byte. A
  `switch` converts both ways, and the compiler checks that it covers every constant:

```java
enum Color { RED, GREEN, BLUE }

string nameOf(Color c) {
    return switch (c) {
        case RED => "red";
        case GREEN => "green";
        case BLUE => "blue";
    };
}

i64 indexOf(Color c) {
    return switch (c) {
        case RED => 0;
        case GREEN => 1;
        case BLUE => 2;
    };
}

void main() {
    Color c = Color.GREEN;
    println("${nameOf(c)} ${indexOf(c)}");
}
```

```output
green 1
```

- **No constants** (limit, with `final` undecided). A static such as `u8 GRASS = 1;` is a
  variable. The compiler accepts one as a `case` label, which the grammar allows only for
  literals and enum constants, and compares with its value when the `switch` runs; and as an
  array's cap, which the C backend doesn't generate (below). Write literals in `case`
  labels, with a comment, or use an enum.
- **Stored lambdas that capture** (undecided, [DESIGN
  §9.3](../DESIGN.md#93-function-types-and-lambdas-decided)). A lambda that uses a local can
  be passed as an argument, and not kept in a variable or field, since a stored `func` is a
  plain function pointer. Put what it needs in a class's fields and call a method, or pass
  the lambda down to where it is used:

```java
void main() {
    i64 limit = 2;
    func(i64 => bool) big = (i64 x) => x > limit;
    println("${big(3)}");
}
```

```error
example.zn:3:29: error: a lambda that captures variables can't be stored in `big`
```

- **A nullable function value that holds a function** (not built). `func(i64 => i64)? g =
  (i64 x) => x + 1;` passes `zink check`; the C backend supports only `null` in one so far.
  Keep a plain `func` whose default is a lambda that does nothing, or a `bool` beside it.
- **Ordering by fields** (undecided): whether `implements ordered` without a `compareTo`
  orders a class field by field. Today it is `` `Point` implements `ordered`, so it needs a
  `compareTo` method ``.
- **A stricter `x @failed = f();`** (undecided), which would make `x` usable only where the
  call succeeded. Today `x` holds the zero value after a failure ([Errors and
  statuses](07-errors.md)).
- **A `flag` statement** to mark a status now and return it later (undecided, DESIGN
  Appendix A.8). `return #(cond) x;` covers the cases seen so far.

### Programs and files

[Programs, packages and building](10-programs.md) shows each of these.

- **Programs of several files** (not built). `zink run`, `zink build` and `zink c` read one
  file, and `zink check` checks each file it is given as a program of its own. An import
  of a program's own package brings in nothing, and there is no `--source-path` and no
  library search path (undecided). Write a program as one script file.
- **A class's `static main` as the entry point** (not built). Only a top-level `main` in a
  script file starts a program; for a package file the C compiler fails at linking with no
  `main`.
- **The rules on files** (not built): that a `package` line matches the file's directory,
  that a package file has one public declaration named after the file, that a script file
  has no `package` line and no access modifiers. None is checked yet.
- **The rules on imports** (not built). Two imports of one name, and an import that clashes
  with a declaration of the file, aren't errors; the first import of a name wins, even over
  a later explicit import or alias, where the design has the explicit one win. Unused
  imports draw no warning. An import of a package that doesn't exist, outside `zink.*`, or
  of a name a library package doesn't have, is accepted, and so is an import of another
  package's non-public class. A full name in a type, such as `zink.json.JsonWriter w`,
  isn't checked when the program doesn't have that package, and fails only when C is
  generated.
- **Full names in expressions** (not built): `zink.io.path.join(a, b)` is ``unknown name
  `zink` ``. A full name works in a type.
- **Static imports, and aliases for whole packages** (undecided). `import static ...` is a
  syntax error; `import zink.io as io;` is accepted and brings in nothing.
- **Program-wide settings** (not built). `#max_memory_mb` and `#data_stack_gb` are parsed
  and have no effect: each data stack reserves 8 GB of address space and nothing limits
  committed memory. An unknown or repeated setting isn't an error yet, and there are no
  build flags to override one. How to enforce `#max_memory_mb` without a check on every
  growth is undecided: committing memory in chunks from the page-fault handler, or sizing
  each stack's reservation.
- **Build flags for `#if`** (not built): a debug build, which would make `debug` true, and
  `--define NAME`. Today `release`, `os(linux)` and `arch(x86_64)` are true and every other
  name is false, on any machine.
- **Static initialization by dependency** (not built). Statics are initialized in the order
  they are written, so an initializer that reads a later static reads its zero value, and
  initializers that read each other in a loop aren't an error. Write each static after the
  ones it reads.

## Arrays and text

### Undecided

- **Slices of cursor, chunked and reversed arrays on data stacks** (`T[~]`, `T[.~.]`,
  `T[~$]`, `T[>..]`). Their elements aren't in order in one block. Capped arrays, rings and
  gap buffers included, can be sliced. Copy the elements into a plain array to slice them:

```java
void main() {
    u8[~] line = [];
    line +^ 'h';
    line +^ 'i';
    string s = line[0:2];
    println(s);
}
```

```error
example.zn:5:16: error: `u8[~]` doesn't keep its elements in order in one block, so it can't be sliced
```

- **Removing from an array inside a for-each by value over it.** Today it is an error that
  suggests `retain`, or a repeat loop:

```java
void main() {
    i64[..~] xs = [1, 2, 3, 4];
    for (i64 x : xs) {
        if (x % 2 == 0) {
            xs ->;
        }
    }
}
```

```error
example.zn:5:13: error: `->` removes from `xs` inside a for-each over it
```

```java
void main() {
    i64[..~] xs = [1, 2, 3, 4];
    xs.retain((i64 x) => x % 2 == 1);
    println("${xs}");
}
```

```output
[1, 3]
```

- **Varint lengths on arrays that grow**, `T[..>*]`. `*` is allowed only on frozen arrays
  today ([DESIGN §4.6](../DESIGN.md#46-varint-lengths-decided)).
- **A second map type with Goose's direct layout**, for large maps that are mostly read:
  faster lookups beyond the CPU cache, in no fixed order, with references that last only to
  the next insert ([DESIGN §15](../DESIGN.md#15-performance)).
- **Caps given by compile-time constants as type arguments** (above, "Syntax").

### Methods that type-check and aren't generated

`zink check` accepts each of these, and the C backend says ``the C backend doesn't support
... yet``. All are designed in STDLIB.md.

| Area | Not generated yet | What to write today |
|---|---|---|
| sorting and searching | `shuffle`, `binarySearch`, `lowerBound`, `upperBound` and their `By` forms, `minIndexBy`, `maxIndexBy` | a loop; `sort` and `partialSort` for order |
| heaps ([STDLIB §7.2](../STDLIB.md#72-heaps-v1)) | `heapPush`, `heapPop`, `heapify` | `partialSort`, or a sorted array |
| on arrays of strings, enums or classes | `indexOf`, `contains`, `min()`, `max()` | `findIndex` and `any` with a lambda |
| `map` | a lambda that returns an array | build the arrays in a loop |
| text | `findAny`, `equalsIgnoreCase`, `replaceAll`, `replaceFirst`, `pad`, `padEnd`, `codePointCount`, `parts.join(sep)` | a loop; compare `toLower()` copies; append to a buffer; count with `codePoints()` |
| code points | `appendUtf8`, `utf8()` on a `u32` array, and reading `utf8()`'s status (a value that isn't a code point gives U+FFFD and no status) | `utf8()` on each code point, after testing its range |
| numbers | `sci(d)`, `isAlnum()`, `isHexDigit()`, `isPrint()` on bytes | `fixed(d)`; tests on the byte's range |
| `math` | `math.gcd`, `math.lcm`, `math.ipow`, `math.isqrt` | a few lines of your own |
| printing | arrays of arrays, of enums and of classes | print the elements in a loop |
| caps | a cap given by a static, `u8[..~CAP]` | write the number |

For example, `join`:

```java
void main() {
    string[..>] parts = "a b c".split(" ");
    u8[.] line = parts.join(",");
    println(line);
}
```

```error
example.zn:3:18: error: the C backend doesn't support `join` on arrays yet
```

```java
void main() {
    string[..>] parts = "a b c".split(" ");
    u8[..>] line = [];
    for (i64 i = 0; i < parts.len; i++) {
        if (i > 0) {
            line.append(",");
        }
        line.append(parts[i]);
    }
    println(line);
}
```

```output
a,b,c
```

### Limits

- **A for-each over a temporary array** is refused as a reference into a temporary, which is
  released at the end of its statement. Keep the array in a variable, or use the lazy form:

```java
void main() {
    string csv = "a,b,c";
    for (string field : csv.split(",")) {
        println(field);
    }
}
```

```error
example.zn:3:25: error: this refers into a temporary value, which is released at the end of the statement
```

```java
void main() {
    string csv = "a,b,c";
    string[..>] fields = csv.split(",");
    for (string field : fields) {
        println(field);
    }
    for (string field : csv.splitBy(",")) {
        println(field);
    }
}
```

```output
a
b
c
a
b
c
```

- **No array of n copies of a value.** A fixed array, `T[.k] a = [];`, starts as k zero
  values, and `fill` sets them all; an array whose length is known only at run time starts
  as a loop of pushes:

```java
void main() {
    i64[.8] counts = [];
    counts.fill(1);
    println("${counts}");
    i64 n = 5;
    u8[..>] row = [];
    for (i64 i = 0; i < n; i++) {
        row +> '.';
    }
    println(row);
}
```

```output
[1, 1, 1, 1, 1, 1, 1, 1]
.....
```

- **A capped `u8` array isn't `ordered`**, so it can't be a sort key: `sortBy` with
  `(Row r) => r.name` on a `u8[..~16]` field is an error. Sort with a comparator that
  passes the two fields to a function taking `string`s, which compares them as text.
- **The lint for unused array capabilities** (not built, [DESIGN
  §4.3](../DESIGN.md#43-layouts-the-compiler-picks-decided)): a warning when an array's
  type allows something the code never does, most of all a cursor slot never moved, which
  makes every read take two loads.
- **Case mapping, properties and display width beyond ASCII** (not built), which need the
  Unicode tables: `"é".toUpper()` is `é`
  ([STDLIB §3.5](../STDLIB.md#35-utf-8-and-bytes)).

## Classes

- **`==` and `!=` on class values** (not built), shown above. Compare the fields, or give
  the class a method:

```java
class Point {
    i64 x;
    i64 y;

    bool same(Point o) {
        return x == o.x && y == o.y;
    }
}

void main() {
    Point a = new Point(1, 2);
    Point b = new Point(1, 2);
    println("${a.same(b)}");
}
```

```output
true
```

- **The order of a class that `implements ordered`** (not built). `a < b` on two class
  values, `sort()`, `max()` and `min()` on an array of them are refused, and `max(a, b)`
  makes C that doesn't compile. Call `compareTo` in a comparator:

```java
class Version implements ordered {
    i64 major;
    i64 minor;

    i32 compareTo(Version o) {
        if (major != o.major) {
            return major < o.major ? -1 : 1;
        }
        return minor < o.minor ? -1 : (minor > o.minor ? 1 : 0);
    }
}

void main() {
    Version[..>] vs = [];
    vs +> new Version(1, 10);
    vs +> new Version(1, 2);
    vs.sort((Version a, Version b) => a.compareTo(b) < 0);
    for (Version& v : vs) {
        println("${v.major}.${v.minor}");
    }
}
```

```output
1.2
1.10
```

- **Class keys in a `hashmap` or `hashset`** (not built), and `Handle<T>` keys. Make one
  integer or string key of the fields:

```java
class Cell {
    i32 x;
    i32 y;
}

void main() {
    hashset<Cell> lit = [];
    lit.add(new Cell(3, 4));
    println("${lit.has(new Cell(3, 4))}");
}
```

```error
example.zn:8:5: error: the C backend doesn't support `Cell` as a key yet
```

```java
i64 key(i32 x, i32 y) {
    return ((i64) x << 32) | (i64) (u32%) y;
}

void main() {
    hashset<i64> lit = [];
    lit.add(key(3, 4));
    println("${lit.has(key(3, 4))} ${lit.has(key(4, 3))}");
}
```

```output
true false
```

- **Copying a map, a set or a pool**, and returning one from a function (not built). Pass
  it by reference, `hashmap<string, i64>& counts`.
- **A value with a growable field, copied or as a temporary** (not built): `Bag b = a;`,
  `return bag;`, and `logs +> new Log(0)` where `Log` has a `u8[..>]` field. Build it in
  the variable that keeps it, and pass it by reference:

```java
class Log {
    i64 level;
    u8[..>] text;
}

void main() {
    Log[..>] logs = [];
    logs +> new Log(0);
    println("${logs.len}");
}
```

```error
example.zn:8:13: error: the C backend doesn't support a temporary value with a growable field yet
```

- **Variable-size values in hash maps, sets, pools and tuples** (not built): a class with a
  `u8[.]` or `T[.*]` field as a map's key or value, a set's or a pool's element, or part of
  a tuple result. The C backend reports that it doesn't support them yet. Keep such values
  in an array of variable-size values, and use a `string` field or a capped array where
  they must go into a map.
- **A lambda that captures, passed to a method called through an interface or to a
  constructor** (not built). Call the method on the class itself, or pass the captured
  values as arguments:

```java
interface Source {
    i64 count(func(i64 => bool) keep);
}

class Numbers implements Source {
    i64[..>] xs;

    i64 count(func(i64 => bool) keep) {
        i64 n = 0;
        for (i64 x : xs) {
            if (keep(x)) {
                n++;
            }
        }
        return n;
    }
}

void main() {
    Numbers nums = new Numbers();
    nums.xs +> 1;
    nums.xs +> 5;
    Source& src = nums;
    i64 limit = 2;
    println("${src.count((i64 x) => x > limit)}");
}
```

```error
example.zn:25:26: error: the C backend doesn't support a lambda that captures, passed to a method called through an interface or to a constructor yet
```

- **Generic methods in an interface** (not built), `<T> T pick(T a, T b);`. Make the
  interface generic instead, `interface Picker<T>`.
- **Values of several classes stored inline**, as Goose's enums are (undecided, [DESIGN
  §6.3](../DESIGN.md#63-interfaces-decided)). An array of interface references,
  `Shape&[..>]`, holds references to values kept elsewhere.
- **`packed` classes with arrays, other classes or type parameters** (not built, [DESIGN
  §6.1](../DESIGN.md#61-classes-decided)). Their fields are numbers, `bool`, enums,
  references, links, slices, functions and other `packed` classes.
- **Arrays of links** (not built in part): `T&<i32>[..~k]` takes `+>`, indexing, for-each,
  `.len` and `clear` so far; not `push`, the removals that give an element back, slices, or
  passing the array whole. A constructor that can fail isn't generated for a value with
  self-relative links ([Memory and references](05-memory.md)).

## Statuses and failures

- **Every `errno` code under its C name** (not built, [DESIGN
  §7.7](../DESIGN.md#77-named-codes-decided)). Only the names in DESIGN §7.7's table are
  known, so `status.ESRCH` is an error; write the number, `#3`:

```java
void main() {
    u8 code = status.ESRCH;
    println("${code}");
}
```

```error
example.zn:2:22: error: `status.ESRCH` isn't a status code
```

- **`@@` on a variable-size record built in place** (not built), as `Item it @@bad =
  make(...)` where `Item` has a `u8[.]` field. `Item it @bad = make(...);` works, so read
  the status with `@` where the call is.
- **A stop on stack overflow** (not built, [DESIGN
  §12.3](../DESIGN.md#123-stacks-and-failures)). The design ends the call stack and every
  data stack in a guard page, and stops the program with a message when one is touched.
  Today the runtime has no handler: a recursion too deep ends in a segmentation fault, exit
  status 139 from the shell, with no message. Use an explicit stack in an array for deep
  walks, as `examples/dirwalk.zn` does.
- **The lint for a `File` that may still be open** at the end of the scope that opened it
  (not built, [DESIGN §6.4](../DESIGN.md#64-resources-and-cleanup-no-destructors-decided)).
  Close with `defer f.close();` right after opening; the runtime closes what is left at
  exit.

## Memory and references

These are the open questions of [REFERENCES §12](../REFERENCES.md#12-open-questions), each
undecided, and what is not built of the rules ([Memory and references](05-memory.md)).

- **Roots that follow the program's order** (undecided). A variable's roots are everything
  stored into it anywhere in its scope, so `w = inner;` inside a block, with `w` declared
  outside it, is an error even when `w` isn't used after the block. Declare the view
  inside the block.
- **Data stacks chosen at run time** (undecided). They would let a recursive function keep
  a growable local; today that is an error, `` `seen` is growable and in scope during this
  call, which can recurse back into `count` ``. Pass the array in by reference from outside
  the cycle.
- **Removal inside a for-each by value**, and **slices of cursor arrays** (both above).
- **Links between two parameters' arrays** (undecided). Rule L1 rejects a link whenever the
  compiler can't tell the arrays apart.
- **Saving and loading values with links as bytes** (undecided), with a check that rejects
  malformed links, like Goose's `from_bytes`.
- **References a host holds into an embedded script's data** (undecided), with embedding
  itself (below, "Tools").
- **Rule G's check that nothing grows an array while text is built in it** (not built).
- **L4's width check left out** when an array's reservation is small enough that every
  offset fits (not built), an optimization that doesn't change what a program does.

## Calling C

[Calling C](12-calling-c.md) covers what works. These are undecided ([DESIGN
§17](../DESIGN.md#17-open-decisions), "Graphics"):

- **Function pointers**, for callbacks such as `qsort`'s comparator: `` func(void => void)
  can't cross into C ``.
- **Structs by value**, as arguments and results, as `div`'s `div_t`.
- **C strings that C returns**, as `getenv`'s `char *`: `` `getenv` can't be called as its
  C header declares it: it returns a `char *`, whose owner and length C doesn't say ``. Use
  the library where it has the function, `env.get("HOME")`; a pointer can only be declared
  as an integer result.
- **C's global variables**, such as `optind`: `unknown name`.
- **A C layout for classes**, with C's field order, alignment and padding, and GLSL's
  `std140` and `std430`, so a slice of a class could be uploaded as it is. Read a C struct
  as an array of numbers by its offsets, as `quarry` reads `SDL_Event`.
- **Vector and matrix types with arithmetic**, such as `f32x4` or `vec3`, since there is no
  operator overloading.
- **Embedding a file at compile time**, such as a SPIR-V shader.
- **Where `vendor.*` packages come from**: shipped with the compiler or fetched on first
  use. None exists yet, and `import vendor.sdl3.*;` is accepted with no warning and brings in
  nothing.

And these are limits of what crosses today:

- **`char **`**, an array of strings: `` string[:] can't cross into C ``. So `execv` and
  `glShaderSource` can't be called.
- **`extern import` takes no defines**, so a function a header declares only under a macro,
  as `gl.h` does under `GL_GLEXT_PROTOTYPES`, needs an explicit declaration.
- **A function-like macro is not a name**: `WEXITSTATUS(s)` is an unknown function. Write
  what it expands to, `(s >> 8) & 0xFF`.

## Threads

[Threads](11-threads.md) covers what works.

- **Workers that read the parent's arrays in place** (undecided), as Goose's research note
  on fork-join child threads proposes, for kernels that split one large array, where
  copying it through queues costs more than the threads gain.
- **Variable-size values through queues** (undecided), such as a class with a `u8[.]`
  field, which is one run of bytes and could be copied whole. Today a queue's values are
  flat, so text travels in a capped array, its whole capacity per message:

```java
import zink.thread.*;

class Line {
    u8[.] text;
}

void main() {
    queue<Line> lines = [];
}
```

```error
example.zn:8:5: error: a queue's values must be flat, and `Line`'s field `text`: `u8[.]` is on one of this thread's data stacks
```

```java
import zink.thread.*;

class Line {
    u8[..~64] text;
}

void main() {
    queue<Line> lines = [];
    Worker w = thread.spawn(() => {
        Line l = new Line("from a worker");
        lines.put(l);
    });
    w.join();
    Line got = lines.get();
    println(got.text);
}
```

```output
from a worker
```

- **A queue lasts for the whole run** (limit). It is never freed, so make queues once, not
  one per message.
- **Method references as the work**, `thread.spawn(this::run)` (undecided). Write
  `thread.spawn(() => run())`.

## The library

### Packages that aren't written

Each is designed in [STDLIB.md](../STDLIB.md) and not written. Importing one is the
warning `` `zink.toml` isn't written yet, so the names this file uses from it aren't
type-checked ``, and a program that uses a name from it doesn't compile to C.

| Package | What it would hold |
|---|---|
| `zink.toml` | `TomlParser`, TOML 1.0.0 into the shared tree ([STDLIB §9.5](../STDLIB.md#95-toml-zinktoml-later)) |
| `zink.yaml` | `YamlParser`, YAML 1.2.2 with the core schema ([STDLIB §9.6](../STDLIB.md#96-yaml-zinkyaml-later)) |
| `zink.regex` | `Regex`, linear-time matching with a Pike VM ([STDLIB §9.9](../STDLIB.md#99-glob-patterns-and-regular-expressions-later)) |
| `zink.glob` | `glob.match` and `fs.glob` (same section) |
| `zink.net` | TCP, then UDP, name lookup, an HTTP/1.1 client ([STDLIB §10.1](../STDLIB.md#101-networking-zinknet)) |
| `zink.compress` | gzip, deflate and zlib, then zstd ([STDLIB §10.2](../STDLIB.md#102-compression-zinkcompress)) |
| `zink.bigint` | `BigInt`, variable-size ([STDLIB §10.4](../STDLIB.md#104-big-integers-zinkbigint)) |
| `zink.test` | `assertEq` and the others, and `zink test` ([STDLIB §10.5](../STDLIB.md#105-tests-zinktest)) |
| `vendor.glfw`, `vendor.sdl3`, `vendor.gl`, `vendor.vulkan` | bindings for windows, input and the GPU ([STDLIB §10.6](../STDLIB.md#106-graphics-vendorglfw-vendorsdl3-vendorgl-vendorvulkan)); not standard library, and no warning |

Until then: run an external program through `zink.os`'s `process` for what a package would
do, call a C library through `extern import` ([Calling C](12-calling-c.md)), as
`examples/quarry` calls SDL2 and OpenGL, and test with programs that check their own
output.

### What written packages don't have yet

- **`hash.xxh3`**, which waits for a reference implementation to test against, and BLAKE3,
  HMAC, SHA-1 and MD5, which come later ([STDLIB §8.3](../STDLIB.md#83-hash)). Use
  `hash.fnv1a`, `hash.crc32` or a SHA-2 or SHA-3 function:

```java
import zink.hash.*;

void main() {
    u64 h = hash.xxh3("abc", 0);
    println("${h}");
}
```

```error
example.zn:4:18: error: `hash` has no function `xxh3`
```

- **JSON5**: `JsonParser`'s and the writer's `json5` field ([STDLIB
  §9.4](../STDLIB.md#94-json5-later)).
- **`TreeBuilder`**, which builds a tree from program data ([STDLIB
  §9.7](../STDLIB.md#97-writing)): `unknown type`. Write with `JsonWriter`, event by event.
- **Reading into classes**, `cfg.into<Config>()`, which needs compile-time access to a
  class's fields; open in [STDLIB §11](../STDLIB.md#11-open-questions).
- **`fs.move` of a directory across file systems** fails with `CROSS_DEVICE`. Copy the
  tree with `fs.walk` and `fs.copy`, then `fs.removeAll`.
- **Processes**: pipelines, pipes to and from a spawned child, and signal handlers
  ([STDLIB §5.1](../STDLIB.md#51-running-programs-process-v1-zinkos)). Run `sh -c` with a
  pipeline in it, or `Command.output` and feed the text to the next `Command`'s `input`.
- **The terminal**: raw mode, reading keys, reading a password without echo; `term` has
  only `size()` ([STDLIB §5.2](../STDLIB.md#52-the-terminal-term-v1-zinkos)).
- **Time**: time zones beyond UTC and fixed offsets, and formatting with patterns
  ([STDLIB §5.3](../STDLIB.md#53-time-zinktime-v1)).
- **`Args` subcommands** ([STDLIB §6.1](../STDLIB.md#61-arguments-args)). Switch on the
  first positional and give the rest to a second `Args`.
- **Sorted walks**: `fs.walk` gives each directory's entries in the file system's order,
  and sorting them needs more buffer space, an open question
  ([STDLIB §11](../STDLIB.md#11-open-questions)). Collect the paths and sort them.
- **`sortedmap` and `bitset`** ([STDLIB §7.6](../STDLIB.md#76-later-containers)): `unknown
  type`. Sort a map's keys into an array, or use a `u64` array as bits.
- **The Unicode tables**, for case mapping and properties beyond ASCII (above, "Arrays and
  text").

The library's other open questions, such as which tier the data formats are in and the
scratch space of `stableSort`, gate nothing ([STDLIB §11](../STDLIB.md#11-open-questions)).

## Tools

- **Programs of several files**, `--source-path` and the library search path (above,
  "Programs and files").
- **Build flags**: `--define`, `--max_memory_mb=...` and a debug build (above).
- **`zink test`** (not built), with `zink.test`: `` zink: unknown command `test` ``.
- **A built-in TinyCC for `zink run`** (undecided, maybe later). It would cut the first run
  after an edit from about a quarter of a second to tens of milliseconds, and need no
  installed C compiler, but it lacks `__int128` and the `__builtin_add_overflow` family the
  generated C uses. Caching a compiled runtime, so a first run compiles only the program's
  own code, is cheaper to try first, and isn't built either.
- **A direct machine-code backend** (undecided), only if depending on a C compiler becomes
  a problem.
- **Other targets** (not built). The compiler compiles for Linux on x86-64: `#if` sees
  `os(linux)` and `arch(x86_64)` wherever it runs, and only clang on x86-64 builds the wider
  versions of `simd` functions.
- **Embedding** (undecided, [DESIGN
  §13](../DESIGN.md#13-embedding-in-other-programs-open)): TinyCC inside the host,
  precompiled `.so` plugins, a bytecode VM, or none. `zink` builds executables only.

## Accepted today, and wrong

The compiler is meant to refuse what it can't compile, with a clear error, rather than make
wrong code. These pass `zink check` today and then do something other than the design says:

- **A static as a `case` label** is accepted, though the grammar allows only literals and
  enum constants, and compared with its value when the `switch` runs (above, "No
  constants").
- **The rules on packages and imports** that [Programs, packages and
  building](10-programs.md) lists as not checked yet: a `package` line in a script file, a
  file's package and its directory, and an import of a `zink.*` package the program doesn't
  have.
- **Failures nothing reports**: `stdout.write` never fails, and a file left open that fails
  when it's written out at exit isn't reported ([System library](13-library-system.md)).
- **A `Duration` overflow** stops the program with a message that names the library's line,
  not the program's ([System library](13-library-system.md)).

Those found while this documentation was written, and fixed since, include: arrays of
another layout passed to a `T[..>]&` or `T[..>]` parameter, which get a copy of the
function for their layout; copies of values with a growable field, refused; failing
constructors nothing reads, which stop the program; field initializers reading fields not
yet set, refused; `==` on arrays of strings and floats, element by element; NaN last in
`min`, `max` and sorts; and the programs whose C didn't compile.

## Listed as missing elsewhere, and there now

Older notes in the design documents and `examples/NOTES.md` call these missing. They work
today:

- **Windows**, `T[<.<]`, which DESIGN §17 says nothing creates: an array literal or another
  array makes one, and `-<` and `->` narrow it.
- **A fixed array of k zero values**, `T[.k] a = [];`, which with `fill` covers the arrays
  of n copies that `examples/NOTES.md` found missing, when n is known at compile time.
- **A static as a `case` label**, which the notes from `quarry` say can't be one. The
  compiler accepts it now, compared at run time, though the grammar doesn't allow it
  (above, "No constants").
- **`split`, and `append` on a capped array**, which `examples/NOTES.md` records as fixed
  while `quarry` and `hashtree` were written.
- **A lambda that captures, passed to a function of the program's own**: the function gets
  a copy with the lambda inlined. Only a method called through an interface, and a
  constructor, can't take one yet (above, "Classes").
