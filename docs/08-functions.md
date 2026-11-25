# 8. Functions and lambdas

This chapter covers functions: declaring them, their parameters and results, overloading,
calling a function as if it were a method, and recursion. Then lambdas and function types,
lambdas that fail, and functions that return several results. Methods, which belong to a
class and see its fields, are in [Classes, interfaces and generics](06-classes.md), and
statuses, which every call returns beside its value, in [Errors and statuses](07-errors.md).
The rules for what a reference may point at, which decide much of what a parameter can be,
are in [Memory and references](05-memory.md).

## Declaring functions

A function has a result type, a name, parameters in parentheses, and a body. In a script
file, functions are written at the top level; in a class, a `static` method is a function
that has no object.

```java
class Temperature {
    static final f64 FREEZING = 0.0;

    static f64 toFahrenheit(f64 celsius) {
        return celsius * 9.0 / 5.0 + 32.0;
    }

    static bool isFreezing(f64 celsius) {
        return celsius <= FREEZING;
    }
}

f64 average(f64 a, f64 b) {
    return (a + b) / 2.0;
}

void report(f64 celsius) {
    println("${celsius} C is ${Temperature.toFahrenheit(celsius)} F");
}

void main() {
    report(average(20.0, 25.0));
    report(later());
    println("${Temperature.isFreezing(-1.5)}");
}

f64 later() {
    return -40.0;
}
```

```output
22.5 C is 72.5 F
-40.0 C is -40.0 F
true
```

- **Order doesn't matter.** A function can call one declared after it.
- **A script file's top-level functions and fields** are the static members of a hidden
  class, so they work as static members do everywhere
  ([DESIGN §8.6](../DESIGN.md#86-script-files-decided)). They are static without saying
  so, and the grammar gives them no `public`, `private` or `static`
  ([GRAMMAR §4](../GRAMMAR.md#4-declarations)), though the compiler doesn't reject those
  yet.
- **A class's static methods** are called as `Temperature.toFahrenheit(x)` from outside the
  class, and by their bare names inside it.
- **`void`** means the function returns nothing. `return;` leaves it early.
- **There are no default parameter values and no varargs.** Overloads (below) cover the
  first; varargs are undecided ([GRAMMAR §8](../GRAMMAR.md#8-not-in-v2-open)).
- **Functions can't be nested.** A lambda (below) is the way to write one inside another.

A top-level field of a script file is a static variable, which every function of the file
can read and change:

```java
i64 lookups = 0;

i64 find(i64[:] xs, i64 want) {
    lookups++;
    for (i64 i = 0; i < xs.len; i++) {
        if (xs[i] == want) {
            return i;
        }
    }
    return -1;
}

void main() {
    i64[..>] xs = [4, 8, 15];
    println("${find(xs, 8)} ${find(xs, 16)} ${lookups}");
}
```

```output
1 -1 2
```

## Parameters

A parameter is either a value or a reference, and its type alone says which. Nothing marks
an argument at the call.

### Plain parameters are read-only copies

A plain parameter is a copy of the argument, and the function can't change it:

```java
void reset(i64 n) {
    n = 0;
}

void main() {
    i64 count = 5;
    reset(count);
}
```

```error
example.zn:2:5: error: can't assign to this: `n` is a plain parameter, which is a read-only copy
help: declare it `i64& n` to let the function change the caller's value (DESIGN §5.1)
```

- **Since the copy can't change**, a function never has to wonder whether it was given a
  copy or the original, and a caller's variable never changes behind its back.
- **To work with a value it was given**, a function copies it into a local:
  `i64 m = n; m++;`.
- **The same holds for a class value** (`p.x = 1` on a plain `Point p` is an error), and
  for an array (`xs +> 1` on a plain `i64[..>] xs` is one), and a plain parameter of a class
  can only call its methods that aren't `mut`
  ([Classes, interfaces and generics](06-classes.md)).

### `T&` changes the caller's value

A parameter whose type ends in `&` is a reference to the caller's variable, field or
element, and the function can change it:

```java
class Point {
    i64 x;
    i64 y;
}

void bump(i64& n) {
    n++;
}

void swap(i64& a, i64& b) {
    i64 t = a;
    a = b;
    b = t;
}

void moveRight(Point& p, i64 dx) {
    p.x += dx;
}

void main() {
    i64 count = 1;
    bump(count);
    Point p = new Point(1, 2);
    i64[..>] xs = [10, 20];
    bump(xs[0]);
    swap(p.x, p.y);
    moveRight(p, 100);
    println("${count} ${p.x} ${p.y} ${xs}");
}
```

```output
2 102 1 [11, 20]
```

- **The call doesn't mark the argument.** `bump(count)` binds `count` to `i64& n`, and only
  the declaration says it may change. Writing `&count` at the call means the same, and draws
  a warning, to help those used to Swift, C# or Rust:

```java
void bump(i64& n) {
    n++;
}

void main() {
    i64 count = 1;
    bump(&count);
    println("${count}");
}
```

```warning
example.zn:7:11: warning: `&` isn't needed at the call
help: the parameter's declaration says whether the argument may change (DESIGN §5.1)
```

- **A reference needs something to refer to**: a variable, a field or an element. A
  literal or a computed value is an error:

```java
void bump(i64& n) {
    n++;
}

void main() {
    i64 count = 1;
    bump(count + 1);
}
```

```error
example.zn:7:10: error: a reference needs a variable, field or element to refer to, and this is a temporary value
help: store the value in a variable first, or copy it with a plain type (DESIGN §5.1)
```

- **Inside the function, the reference is used like the value**: no `*` and no `->`.
  `n++` changes the caller's `count`.
- **A function can return a reference** rooted at a `&` or slice parameter, as `find` in the
  [tour](01-getting-started.md#a-tour-of-a-larger-program) does; never one to its own
  local. What a reference can point at, and for how long, is
  [Memory and references](05-memory.md).

### Arrays and slices

An array parameter says, by its type, what the function does with the array:

| Parameter | What the function gets | It can |
|---|---|---|
| `i64[:] xs` | a read-only view of any array of `i64`, or of part of one | read |
| `i64[:]& xs` | a writable view: elements can change, the length can't | read and write elements |
| `i64[..>]& xs` | a reference to the caller's growable array | do whatever `[..>]` allows: append |
| `i64[..>] xs` | a read-only copy of an array of that kind | read |

```java
i64 sum(i64[:] xs) {
    i64 total = 0;
    for (i64 x : xs) {
        total += x;
    }
    return total;
}

void addSquares(i64[..>]& out, i64 n) {
    for (i64 i = 1; i <= n; i++) {
        out +> i * i;
    }
}

void zeroFirst(i64[:]& xs) {
    xs[0] = 0;
}

void main() {
    i64[..>] squares = [];
    addSquares(squares, 4);
    i64[.3] fixed = [7, 8, 9];
    println("${squares} ${sum(squares)} ${sum(squares[1:3])} ${sum(fixed)}");
    zeroFirst(squares);
    zeroFirst(fixed);
    println("${squares} ${fixed}");
}
```

```output
[1, 4, 9, 16] 30 13 24
[0, 4, 9, 16] [0, 8, 9]
```

- **`T[:]` is the usual parameter for reading.** Every kind of array converts to a slice of
  its elements, so `sum` takes a growable array, a fixed one or a slice of either. `string`
  is `u8[:]`, so a `string` parameter takes text in any form.
- **`T[:]&` is a writable view**, for a function that changes elements but not the length,
  as a sort does. A read-only slice, such as a `string`, can't become one: passing `"abc"`
  to a `u8[:]&` parameter is an error.
- **`T[..>]&` and the other array references** let the function grow or shrink the
  caller's array, as far as its slots allow ([Arrays](04-arrays.md)). Their slots are a
  contract: a function that takes `T[..>]&` can only append to what it's given.
- **A plain array parameter, `T[..>] xs`, is a read-only copy**, and adds nothing to a
  `T[:]`, which takes any kind of array. Prefer `T[:]`. Today the checker lets a plain array
  parameter take an array of any kind, and the C backend then fails on a fixed, capped or
  double-ended one.

`main`'s `string[.] args` is a plain parameter too: `args[1:].sort()` in `main` is an
error, since sorting would change it.

The rules are in [DESIGN §5.1](../DESIGN.md#51-copies-and-references-decided) and
[Memory and references](05-memory.md#parameters), and what each array kind allows in
[Arrays](04-arrays.md).

## Results

A function returns one value of its result type, several values as a tuple (below,
"Multiple results"), or nothing.

```java
u8[.] label(string name, i64 n) {
    return "${name}=${n}";
}

i64[..>] squares(i64 n) {
    i64[..>] out = [];
    for (i64 i = 1; i <= n; i++) {
        out +> i * i;
    }
    return out;
}

string longer(string a, string b) {
    return a.len >= b.len ? a : b;
}

void main() {
    u8[.] l = label("x", 3);
    println(l);
    i64[..>] sq = squares(5);
    sq +> 36;
    println("${sq} ${squares(3)}");
    println(longer("pear", "fig"));
}
```

```output
x=3
[1, 4, 9, 16, 25, 36] [1, 4, 9]
pear
```

- **Every path must return a value**, unless the result is `void`:

```java
i64 sign(i64 n) {
    if (n < 0) {
        return -1;
    } else if (n > 0) {
        return 1;
    }
}

void main() {
    println("${sign(-4)}");
}
```

```error
example.zn:1:5: error: `sign` can reach its end without returning a value
```

- **An array result is built where the caller keeps it.** `i64[..>] sq = squares(5);` has
  `squares` build its `out` in `sq`'s own storage, so nothing is copied, and the caller can
  go on growing it ([DESIGN §3.3](../DESIGN.md#33-building-in-place-decided)).
- **Text a function builds is returned as a `u8` array**, such as `u8[.]`, which holds it.
  A `string` is only a view, so `return "${name}=${n}";` from a function returning `string`
  is an error: the text it would view is a temporary that ends with the `return`
  ([Text](03-text.md)).
- **A `string` result can view text that outlives the call**, as `longer` does: a literal,
  or text the caller passed in. The compiler works out which argument each result can
  point into, so the caller knows how long it lasts, without annotations
  ([Memory and references](05-memory.md)).
- **The status** that every call returns beside its value is set with `return #code value;`
  and read with `@` ([Errors and statuses](07-errors.md)).

## Overloading

Functions, static methods and constructors can share a name when their parameter types
differ. The compiler picks one at compile time, from the arguments' static types:

```java
void show(i64 n) {
    println("i64 ${n}");
}

void show(u64 n) {
    println("u64 ${n}");
}

void show(f64 x) {
    println("f64 ${x}");
}

void show(string s) {
    println("string ${s}");
}

void show(i64 a, i64 b) {
    println("two: ${a}, ${b}");
}

void main() {
    u32 count = 7;
    i32 small = -3;
    f32 ratio = 0.5f32;
    show(42);
    show(count);
    show(small);
    show(ratio);
    show("x");
    show(1, 2);
    show((i64) count);
}
```

```output
i64 42
u64 7
i64 -3
f64 0.5
string x
two: 1, 2
i64 7
```

How a call is chosen:

1. **The candidates** are the declarations with that name and as many parameters as the
   call has arguments.
2. **A candidate fits** when each argument has its parameter's type, or converts to it
   implicitly ([The basics](02-basics.md#implicit-conversions)).
3. **Each argument's fit is ranked**:
   - best is an *exact* match: the same type, or a literal of its default type (`i64` for
     integers, `f64` for floats, `u32` for characters);
   - next is *widening within a kind*: a signed type to a wider signed one, an unsigned type
     to a wider unsigned one, `f32` to `f64`, or a literal to another type it fits;
   - last come *other conversions*: an unsigned type to a wider signed one, an integer to
     a float, a class value to a reference to an interface it implements, and an array to a
     slice of its elements.
4. **The chosen candidate** fits every argument at least as well as every other candidate
   does, and some argument better.
5. **A non-generic candidate beats a generic one** that fits equally well.

So `show(count)` with a `u32` takes `show(u64)`, which keeps it unsigned, over `show(i64)`;
`show(small)` with an `i32` takes `show(i64)`; and `show(ratio)` takes `show(f64)`. When no
candidate is best, the call is ambiguous, and a cast chooses:

```java
void g(i32 a, i64 b) {
}

void g(i64 a, i32 b) {
}

void main() {
    i32 x = 1;
    g(x, x);
}
```

```error
example.zn:9:5: error: the call to `g` is ambiguous
help: it fits void g(i32, i64) and void g(i64, i32); cast an argument to choose (DESIGN §6.5)
```

- **A character literal** is a `u32`, so with no `u32` overload, `show('a')` above is
  ambiguous between `i64`, `u64` and `f64`, which it fits equally well.
- **Overloads must differ in a parameter's type.** Differing only in `&`, `?`, an array's
  slots, the result type or the parameter names is an error, since one call could fit
  both. A slice and an array are different kinds, so `f(string)` beside `f(u8[..>]& b)` is
  allowed, and a buffer argument takes the second, its exact match.

```java
class Item {
    i64 n;
}

void f(Item x) {
}

void f(Item& x) {
}

void main() {
}
```

```error
example.zn:8:6: error: `f` is declared twice with the same parameter types
help: overloads must differ in a parameter's type, not only in `&`, `?`, array slots or the return type (DESIGN §6.5)
```

- **Static types decide.** An argument's declared type picks the overload, never the value
  it holds at run time.
- **Generic and plain overloads mix.** A generic one wins where it fits exactly and the
  plain one only by a conversion:

```java
<T implements number> void describe(T x) {
    println("some number ${x}");
}

void describe(i64 n) {
    println("i64 ${n}");
}

void count(string s) {
    println("a view of ${s.len} bytes");
}

void count(u8[..>]& buf) {
    println("a buffer of ${buf.len} bytes");
}

void main() {
    describe(1);
    describe(2.5);
    u8 b = 3;
    describe(b);
    u8[..>] buf = [];
    buf.append("abc");
    count(buf);
    count("hello");
}
```

```output
i64 1
some number 2.5
some number 3
a buffer of 3 bytes
a view of 5 bytes
```

- **Overloads should do the same thing for different inputs.** Different behavior gets a
  different name, as with the library's `sort(less)` and `sortBy(key)`.

The rules, and the reasons for them, are in
[DESIGN §6.5](../DESIGN.md#65-overloading-decided).

## Calling a function as a method

A function whose first parameter fits a value can be called on that value, as if it were a
method: `x.f(a)` means `f(x, a)`.

```java
class Point {
    i64 x;
    i64 y;
}

i64 manhattan(Point p) {
    return abs(p.x) + abs(p.y);
}

i64 doubled(i64 n) {
    return n * 2;
}

bool isBlank(string s) {
    return s.trim().len == 0;
}

void push2(i64[..>]& xs, i64 v) {
    xs +> v +> v;
}

void main() {
    Point p = new Point(3, -4);
    println("${p.manhattan()} ${manhattan(p)} ${21.doubled()} ${doubled(5).doubled()} ${"  ".isBlank()}");
    i64[..>] xs = [];
    xs.push2(7);
    println("${xs}");
}
```

```output
7 7 42 20 true
[7, 7]
```

- **The functions it finds** are those a bare name would: in a script file, its top-level
  functions; inside a class, also that class's own static methods.
- **A real method comes first.** If the value's class has a method of that name, `x.f()`
  calls it, and `f(x)` calls the function.
- **It reads left to right**, which helps with chains: `doubled(5).doubled()`.
- **It adds no methods to a class** from outside: there is no extending a type, and the
  call means exactly `f(x, ...)`, with the same overloading.

## Recursion

A function may call itself, directly or through others:

```java
i64 fib(i64 n) {
    if (n < 2) {
        return n;
    }
    return fib(n - 1) + fib(n - 2);
}

void digits(i64 n, u8[..>]& out) {
    if (n >= 10) {
        digits(n / 10, out);
    }
    out +> (u8) ('0' + n % 10);
}

void main() {
    println("${fib(30)}");
    u8[..>] text = [];
    digits(90210, text);
    println(text);
}
```

```output
832040
90210
```

### No growable local during a recursive call

A function can't call back into its own cycle while one of its growable locals is in scope.
A growable array, such as a `u8[..>]`, lives on a data stack of its own, and one per
activation of a recursive function isn't something the compiler can assign
([Memory and references](05-memory.md#recursion),
[REFERENCES §8.4](../REFERENCES.md#84-recursion)):

```java
void digits(i64 n, u8[..>]& out) {
    u8[..>] mine = [];
    if (n >= 10) {
        digits(n / 10, mine);
    }
    out.append(mine);
    out +> (u8) ('0' + n % 10);
}

void main() {
    u8[..>] text = [];
    digits(90210, text);
    println(text);
}
```

```error
example.zn:4:9: error: `mine` is growable and in scope during this call, which can recurse back into `digits`
help: a growable local would need a new data stack on every call; pass it in by reference from outside the cycle (REFERENCES §8.4)
```

- **Pass the growable data in by reference** from outside the cycle, as the first `digits`
  does with `out`.
- **Or end its scope before the call.** A local in a block that closes before the recursive
  call is fine:

```java
i64 countDown(i64 n) {
    if (n == 0) {
        return 0;
    }
    i64 here = 0;
    {
        i64[..>] scratch = [];
        scratch +> n +> n;
        here = scratch.sum();
    }
    return here + countDown(n - 1);
}

void main() {
    println("${countDown(3)}");
}
```

```output
12
```

- **What counts**: growable arrays, frozen arrays such as `u8[.]`, variable-size records
  and roots. Numbers, class values of fixed size, references, slices, fixed arrays
  (`i64[.4]`) and capped ones (`u8[..~16]`) live on the native stack, and are fine.
- **Through other functions too.** In `isEven` calling `isOdd` calling `isEven`, a growable
  local of either is in scope during a call that can come back.
- **A growable value can't be passed by value into the cycle**, as in
  `count(i64[..>] xs, ...)` calling itself; take `i64[:]` or `i64[..>]&` instead.
- **The array `filter`, `map` or `sortedIndexes` is building counts as such a local** while
  its lambda runs, so their lambda can't recurse back into the function.

### Depth

Each call takes space on the native stack, usually 8 MB on Linux. A recursion deeper than
that ends the program with a segmentation fault today, without the message the design asks
for ([DESIGN §12.3](../DESIGN.md#123-stacks-and-failures)); at `-O2`, the C compiler turns
some recursion, such as `return n + sumTo(n - 1);`, into a loop. A loop, or data kept in an
array, is the way to go deep.

### The base case's copy

For a function that calls itself at least twice and starts with its base case, such as
`fib`, the compiler gives each call to itself a copy of that test: a call whose argument
already meets it, `fib(1)` or `fib(0)`, computes its result in place instead of making the
call. A tree walk then makes about half as many calls, which took 19% fewer cycles in the
benchmarks' interpreter. It changes nothing a program can see
([DESIGN §14.2](../DESIGN.md#142-what-emitting-c-requires)).

## Lambdas

A lambda is a function written where it's used: parameters with their types, `=>`, and a
body.

```java
class Dog {
    string name;
    i64 age;
}

void main() {
    Dog[..>] dogs = [new Dog("rex", 5), new Dog("ada", 2), new Dog("bo", 7)];
    i64 minAge = 3;
    var adults = dogs.filter((Dog d) => d.age >= minAge);
    println("${adults.len} ${adults[0].name}");
    dogs.sort((Dog a, Dog b) => {
        return a.name < b.name;
    });
    println("${dogs[0].name} ${dogs[1].name} ${dogs[2].name}");
    i64 oldest = dogs.fold(0, (i64 acc, Dog d) => max(acc, d.age));
    println("${oldest} ${dogs.count((Dog d) => d.age > minAge)} ${dogs.any((Dog d) => d.name == "bo")}");
}
```

```output
2 rex
ada bo rex
7 2 true
```

- **Every parameter's type is written**, and the parentheses always are, even for one
  parameter: `(Dog d) => d.age`, never `d => d.age` or `(a, b) => a + b`. A lambda then
  reads the same wherever it appears, needs no type from where it is passed, and lets
  overloads be chosen from argument types alone. `var` isn't allowed as a parameter's type.
- **No parameters** is `() => ...`.
- **An unused parameter is named `_`**: `(i64 a, i64 _) => a`.
- **The body is an expression**, whose value is the result, **or a block**, with `return`s.
  The result type comes from the body.
- **The parameter types must be exactly those of the function type** it is passed as, with
  no implicit conversions:

```java
void main() {
    u8[..>] bytes = [3, 1, 2];
    bytes.sort((i64 a, i64 b) => a < b);
}
```

```error
example.zn:3:16: error: expected func(u8, u8 => bool), found func(i64, i64 => bool)
```

- **`=>` is the lambda arrow, as in C#**, and the `switch` arm's. `->` is an array
  operator ([Arrays](04-arrays.md)).

The design is [DESIGN §9.3](../DESIGN.md#93-function-types-and-lambdas-decided).

### Captures are read-only

A lambda can use the variables in scope where it is written, which it *captures*. It can't
change them, and they can't change after it captures them, as with Java's "effectively
final":

```java
void main() {
    i64 hits = 0;
    i64[..>] xs = [1, 2, 3];
    i64 n = xs.count((i64 x) => {
        hits++;
        return x > 1;
    });
}
```

```error
example.zn:5:9: error: can't change this: `hits` is captured by this lambda, and captures are read-only
help: have the lambda return a value instead (DESIGN §9.3)
```

```java
void main() {
    i64 lim = 1;
    i64[..>] xs = [1, 2, 3];
    i64 n = xs.count((i64 x) => x > lim);
    lim = 2;
}
```

```error
example.zn:5:5: error: `lim` is assigned again after a lambda captured it
help: a captured variable never changes after it's initialized; copy it into a new variable for the lambda (DESIGN §9.3)
```

- **A captured variable is assigned once**: at its declaration, or once on each path, as
  with `if`/`else`. Assigned before the capture and then again, or in a loop, it's an error
  too.
- **Instead of counting in a capture**, return a value, and let the built-in combine them:
  `count`, `fold`, `any` and `all` exist for that.
- **A captured reference can still be written through.** If the lambda captures
  `Item& r`, `r.n++` changes the item; what can't change is which item `r` refers to.
- **A lambda's own parameters are plain parameters**, read-only like any others.

### Passed to a built-in, a lambda is inlined

A lambda passed to a built-in, such as `sort`, `filter`, `map`, `fold`, `count`, `any`,
`all`, `findIndex` or `retain`, is compiled into that built-in's loop, its parameters
pointing at the elements. It costs what a handwritten loop would, and it may capture
anything in scope. [Arrays](04-arrays.md) lists the built-ins that take one. It is checked
as that loop: removing from the array the built-in works on, inside the lambda, is an
error, as it is inside a for-each over it.

What such a lambda returns is part of what the built-in gives, so it may return text built
for the statement, which the statement then uses:

```java
void main() {
    string[..>] words = ["a", "b"];
    println("${words.map((string s) => "<${s}>")}");
}
```

```output
[<a>, <b>]
```

### Passed to a function of the program's own

A function can take a function as a parameter, with a function type such as
`func(i64 => i64)` (below). A lambda passed to it may capture, too:

```java
i64 applyTwice(func(i64 => i64) f, i64 x) {
    return f(f(x));
}

i64 countWhere(i64[:] xs, func(i64 => bool) keep) {
    i64 n = 0;
    for (i64 x : xs) {
        if (keep(x)) {
            n++;
        }
    }
    return n;
}

void each(i64[:] xs, func(i64 => void) f) {
    for (i64 x : xs) {
        f(x);
    }
}

void main() {
    i64 step = 10;
    println("${applyTwice((i64 x) => x + step, 1)}");
    i64[..>] xs = [4, 8, 15, 16, 23, 42];
    println("${countWhere(xs, (i64 x) => x > step)}");
    string prefix = "item";
    each(xs[0:2], (i64 x) => println("${prefix} ${x}"));
}
```

```output
21
4
item 4
item 8
```

- **Each lambda that captures gets a copy of the function** it is passed to, with the lambda
  inlined where the function calls it, and pointers to the variables it reads. So
  `countWhere` costs what a loop written for `x > step` would. A copy that passes the lambda
  on, to a built-in, another function or itself, passes it on inlined too.
- **A function that wraps the lambda in a new one at every call to itself** would need
  copies without end, as in `nest(n - 1, (i64 y) => f(y) + 1)` inside `nest`. The C backend
  reports it as not supported.
- **Such a lambda can't return text built for the statement**, since the function it goes
  to may keep the result past it: `each(words, (string s) => "<${s}>")` with a
  `func(string => string)` parameter is an error. Only a built-in can take one. Nor can it
  return a view of a buffer it captures, for the same reason; a captured view of a string
  literal is fine.
- **Its parameters are what the function passes it**, which may last only for the call:
  the lambda's own locals can keep them, and nothing outside it can.
- **Calling a function value does what the lambdas it may be do**: a call `f(xs)` through a
  `func(i64[..~]& => void)` counts as removing from `xs` when any lambda with one parameter
  in the program removes from its parameter, so a reference into `xs` can't be used after
  it.
- **Not yet**: a lambda that captures, passed to a constructor or to a method called through
  an interface. The C backend says so; a lambda that captures nothing works there
  ([What isn't there yet](15-not-yet.md)).

### A stored lambda can't capture

A lambda kept in a variable, field or array is a plain function pointer, so it can't
capture:

```java
void main() {
    i64 k = 1;
    func(i64 => i64) f = (i64 a) => a + k;
}
```

```error
example.zn:3:26: error: a lambda that captures variables can't be stored in `f`
help: only a lambda passed as an argument can capture; a stored `func` is a plain function pointer (DESIGN §9.3)
```

- **It can use statics**, such as a script file's top-level fields, and call functions:
  they aren't captures.
- **A constructor that stores the lambda it is given**, as the default constructor of a
  class with a `func` field does, gives the same error: `new Filter((i64 x) => x > k)`.
- **Stored closures that capture are undecided**
  ([DESIGN §17](../DESIGN.md#17-open-decisions)): they would need their captures kept
  somewhere that outlives the scope, and there is no heap.

## Function types

`func(A, B => R)` is the type of a function that takes an `A` and a `B` and returns an `R`.
Both sides are always written, with `void` for none: `func(void => void)`,
`func(string => void)`, `func(void => bool)`.

```java
i64 square(i64 x) {
    return x * x;
}

class Op {
    string name;
    func(i64, i64 => i64) apply;
}

void times(i64 n, func(void => void) f) {
    for (i64 i = 0; i < n; i++) {
        f();
    }
}

void main() {
    func(i64 => i64) f = (i64 x) => square(x) + 1;
    println("${f(3)}");
    Op[..>] ops = [];
    ops +> new Op("add", (i64 a, i64 b) => a + b);
    ops +> new Op("max", (i64 a, i64 b) => max(a, b));
    for (Op& op : ops) {
        println("${op.name} ${op.apply(3, 9)}");
    }
    func(i64 => i64)[..>] steps = [];
    steps +> (i64 x) => x + 1;
    steps +> (i64 x) => x * 2;
    i64 v = 5;
    for (func(i64 => i64) s : steps) {
        v = s(v);
    }
    println("${v}");
    times(2, () => println("again"));
    var g = (i64 x) => x - 1;
    println("${g(10)}");
}
```

```output
10
add 12
max 9
12
again
again
9
```

- **A `func` value is called like a function**: `f(3)`, `op.apply(3, 9)`.
- **It can be a local, a parameter, a field or an array element**, and `var` takes the type
  of a lambda.
- **A function's name isn't a value.** `func(i64 => i64) g = square;` is an error that
  says to call it; wrap it in a lambda, `(i64 x) => square(x)`. Method references, such as
  `this::close`, are undecided.
- **`func(...)?` may be `null`**, and must be checked before it is called:

```java
void main() {
    func(i64 => i64)? onDone = null;
    i64 y = onDone(1);
}
```

```error
example.zn:3:13: error: `onDone` may be null here
```

- **Not yet**: the C backend supports only `null` in a nullable function so far. Giving
  one a function, `func(i64 => i64)? g = (i64 x) => x;`, passes `zink check` and is reported
  as not supported by `zink run` ([What isn't there yet](15-not-yet.md)).
- **A function type can return several results**: `func(i64 => (i64, i64))` (below).
- **A `func` value is an 8-byte pointer**, and a call through it is an indirect call.

## Lambdas that fail

A lambda can fail as any function can, with `return #code value;` or `return ## f();`
([Errors and statuses](07-errors.md#statuses-in-lambdas)). Every function type can fail,
so the type doesn't say it:

```java
void main() {
    func(i64 => i64) half = (i64 x) => {
        if (x % 2 != 0) {
            return #status.INVALID 0;
        }
        return x / 2;
    };
    func(string => i64) port = (string s) => {
        return ## i64.parse(s);
    };
    i64 a @e1 = half(10);
    i64 b @e2 = half(7);
    i64 p @e3 = port("80x");
    println("${a} ${e1} ${b} ${status.name(e2)} ${p} ${status.name(e3)}");
    i64[..>] xs = [5, 3, 9, 1];
    u8 why = @xs.sort((i64 x, i64 y) => {
        if (x == 9 || y == 9) {
            return #status.INVALID false;
        }
        return x < y;
    });
    println("${status.name(why)}");
    i64 c = half(3);
}
```

```output
5 0 0 INVALID 0 INVALID
INVALID
```

```stderr exit=1
example.zn:23: `half` failed with INVALID, and nothing read its status
`lambda` returned it at example.zn:4
```

- **A call through a `func` value can fail**: `@` reads its status, and one nothing reads
  stops the program at the call, naming the lambda's line in its trail.
- **A lambda passed to a built-in fails that call** with its status, as `@xs.sort(...)`
  reads here. The built-in stops at once, and may leave what it was changing part-way,
  such as an array half sorted.
- **`return ## e;`** passes on `e`'s value and its status, so `port` fails exactly when
  `i64.parse` does.
- **The cost** is that a `func` returns its status beside its value, as any function that
  can fail does; a built-in's inlined lambda costs nothing more unless it fails.

## Multiple results

A function returns several values by declaring a tuple type, `(T1, T2, ...)`, with two or
more types, and writing `return (e1, e2, ...);`. The caller takes them apart:

```java
(string, string) splitFirst(string s) {
    for (i64 i = 0; i < s.len; i++) {
        if (s[i] == ' ') {
            return (s[:i], s[i + 1:]);
        }
    }
    return (s, "");
}

(i64, i64) divMod(i64 a, i64 b) {
    if (b == 0) {
        return #status.DOMAIN (0, 0);
    }
    return (a / b, a % b);
}

void main() {
    (string cmd, string arg) = splitFirst("open notes.txt");
    println("[${cmd}] [${arg}]");
    (cmd, arg) = splitFirst("quit");
    println("[${cmd}] [${arg}]");
    (string word, _) = splitFirst("first second");
    println(word);
    (i64 q, i64 r) @err = divMod(17, 5);
    println("${q} ${r} ${err}");
    (q, r) @err = divMod(1, 0);
    println("${q} ${r} ${status.name(err)}");
    (var a, var b) = divMod(9, 4);
    i64 x = 1;
    i64 y = 2;
    (x, y) = (y, x);
    println("${a} ${b} ${x} ${y}");
}
```

```output
[open] [notes.txt]
[quit] []
first
3 2 0
0 0 DOMAIN
2 1 2 1
```

- **A destructuring declares its targets**, each with a type or `var`, **or assigns to
  existing variables**, `(cmd, arg) = ...`. The grammar makes a mix of the two an error;
  the compiler accepts one today ([GRAMMAR §5](../GRAMMAR.md#5-statements)).
- **`_` discards one result**, in either form.
- **A status capture goes after the closing parenthesis**: `(i64 q, i64 r) @err = ...`.
  On failure, each result is its zero value, here `(0, 0)`.
- **`(x, y) = (y, x)` swaps**: the right side is evaluated before anything is assigned.
- **A tuple expression**, `(a, b)`, appears only after `return`, as a lambda's expression
  body, and on the right of a destructuring.
- **Tuples aren't values of their own.** There are no tuple variables, fields or array
  elements, and no `t.0`; a call that returns one is destructured, discarded with
  `_ = f();`, or made as a plain statement:

```java
(i64, i64) pair(i64 n) {
    return (n, n + 1);
}

void main() {
    var t = pair(1);
}
```

```error
example.zn:6:13: error: a call with several results must be destructured
help: write `(T a, T b) = f();` (DESIGN §9.5)
```

- **The count must match**: three targets for two results is an error that says
  `this gives 2 results, and there are 3 targets`.
- **They cost nothing extra**: results come back in registers, or are built in place in
  the caller's variables, as a single result is. They cost what a small class with the same
  fields would, and less than scanning twice.
- **Each result has its own roots.** Both of `splitFirst`'s are views into its argument, so
  both last as long as it does ([Memory and references](05-memory.md)).
- **A repeat loop can destructure**, as in `while ((string key, string value) : ini.next())`
  ([Errors and statuses](07-errors.md#repeat-loops)).

### Tuples and lambdas

A function type's result can be a tuple, and a lambda's expression body can be a tuple
expression:

```java
(i64, i64) minMax(i64[:] xs, func(i64 => i64) key) {
    i64 lo = i64.MAX;
    i64 hi = i64.MIN;
    for (i64 x : xs) {
        i64 k = key(x);
        lo = min(lo, k);
        hi = max(hi, k);
    }
    return (lo, hi);
}

(i64, i64) shifted(i64 x, func(i64 => (i64, i64)) split) {
    (i64 a, i64 b) = split(x);
    return (a + 1, b + 1);
}

void main() {
    i64[..>] xs = [3, -7, 5];
    i64 offset = 100;
    (i64 lo, i64 hi) = minMax(xs, (i64 x) => x + offset);
    println("${lo} ${hi}");
    (i64 q, i64 r) = shifted(17, (i64 x) => (x / 5, x % 5));
    println("${q} ${r}");
}
```

```output
93 105
4 3
```

A lambda with a block body can't `return (a, b);` today: the compiler takes the tuple for a
call it expects to be destructured, and rejects it. An expression body, as above, works.

## Generic functions

A function can take type parameters, written before its result type, with bounds that say
what the body may do with them:

```java
<T implements number> T sumOf(T[:] xs) {
    T total = 0;
    for (T x : xs) {
        total += x;
    }
    return total;
}

<T implements ordered> T largest(T[:] xs) {
    T best = xs[0];
    for (T x : xs) {
        if (x > best) {
            best = x;
        }
    }
    return best;
}

<T> T pick(bool first, T a, T b) {
    return first ? a : b;
}

void main() {
    i64[..>] ints = [3, 1, 4];
    f64[..>] floats = [0.5, 2.25];
    string[..>] words = ["pear", "fig", "plum"];
    println("${sumOf(ints)} ${sumOf(floats)} ${largest(words)} ${pick(false, "a", "b")}");
}
```

```output
8 2.75 plum b
```

- **The type arguments are inferred from the arguments.** There is no way to write them at
  the call yet (undecided, [GRAMMAR §8](../GRAMMAR.md#8-not-in-v2-open)).
- **Each set of type arguments gets its own compiled copy**, so a generic function costs
  what one written for that type would.
- **No conversion makes two arguments agree**: `max(small, wide)` with a `u8` and an `i32`
  is an error ([The basics](02-basics.md#min-max-clamp-and-abs)).
- Bounds, generic classes and the rest are in
  [Classes, interfaces and generics](06-classes.md#generics).
