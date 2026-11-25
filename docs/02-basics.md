# 2. The basics

This chapter covers what every program uses: comments and names, literals, the number
types and how values move between them, arithmetic and its three kinds of overflow, casts,
floats, the methods and functions on numbers, `bool` and conditions, variables, operators,
the statements that make up a function body, and `main`. Text has a chapter of its own,
[Text](03-text.md), and so do arrays, [Arrays](04-arrays.md).

Zinkite looks like Java, and most of this chapter will look familiar. What differs is
mostly in the numbers: there are no `int` and `long`, a literal takes its type from where it
is used, arithmetic wraps unless it is written to fail or saturate, and a cast that would
lose a value stops the program instead of truncating it.

## Comments

```java
// A line comment runs to the end of the line.
/* A block comment
   can span lines. */
void main() {
    i64 n = 1 /* or sit inside one */ + 2;
    println("${n}");    // 3
}
```

```output
3
```

- **Block comments don't nest.** The first `*/` ends the comment, so a `/* ... */` inside
  one ends it early.
- `/** ... */` is an ordinary block comment; there are no documentation comments.
- A `/*` that is never closed is an error, which says so.

## Names and keywords

A name starts with an ASCII letter or `_`, followed by letters, digits and `_`: `count`,
`lineNo`, `_tmp`, `x2`. Letters outside ASCII aren't allowed yet (Unicode letters in names
are undecided, [GRAMMAR §8](../GRAMMAR.md#8-not-in-v2-open)). A lone `_` isn't a name: it
discards a value (below, "`_` discards").

The convention, which the library follows: `camelCase` for locals, fields and methods,
`PascalCase` for classes, interfaces and enums, and `UPPER_CASE` for static constants and
enum constants.

These words are reserved:

| Group | Keywords |
|---|---|
| Types | `bool` `i8` `i16` `i32` `i64` `i128` `u8` `u16` `u32` `u64` `u128` `f32` `f64` `string` `void` `func` |
| Declarations | `package` `import` `as` `class` `interface` `enum` `implements` `static` `public` `private` `mut` `final` `extern` |
| Statements | `if` `else` `while` `do` `for` `break` `continue` `return` `switch` `case` `default` `yield` `var` `defer` |
| Expressions | `new` `this` `null` `true` `false` `instanceof` |
| Reserved, not used | `extends` `protected` `abstract` `struct` `super` `try` `catch` `finally` `throw` `long` `short` `byte` `double` `char` `delete` |

The last row is reserved so the compiler can say why each isn't there, for the habits that
Java, C and C++ bring:

```java
void main() {
    long count = 0;
}
```

```error
example.zn:2:5: error: `long` is reserved: the type is spelled `i64`
```

- `short`, `byte` and `double` suggest `i16`, `u8` and `f64`; `char` says that a code point
  is a `u32` ([DESIGN §9.1](../DESIGN.md#91-number-types-and-sizes-decided)).
- `extends`, `super`, `abstract` and `protected` say there is no inheritance
  ([Classes, interfaces and generics](06-classes.md)).
- `try`, `catch`, `finally` and `throw` say that failures are status codes
  ([Errors and statuses](07-errors.md)).
- `delete` says there are no destructors, and suggests `defer`; `struct` suggests `class`.

Some words are keywords only in one place, so they can still name variables and fields:
`root` and `packed` at the start of a type declaration, `simd` before a function, `link`
after `extern`, and `in` inside a link's angle brackets.

`int` and `float` aren't types but generic bounds, as are `sint`, `uint`, `number`,
`ordered` and `hashable` ([Classes, interfaces and generics](06-classes.md)). Used as a
type, `int` and `float` suggest `i64` and `f64`:

```java
void main() {
    int x = 5;
}
```

```error
example.zn:2:5: error: `int` is a generic bound, not a type (DESIGN §10.1)
help: for a type, write `i64` or `i32`
```

The bounds, and the built-in generic types `hashmap`, `hashset`, `pool` and `queue`, can't
name a class, interface or enum of a program's own.

## Literals

### Integers

```java
void main() {
    i64 million = 1_000_000;
    u32 mask = 0xFF_FF;
    u8 flags = 0b1010_0001;
    var small = 255u8;
    var wide = 1i128;
    var h = 0x1f32;
    println("${million} ${mask} ${flags} ${small} ${wide} ${h}");
}
```

```output
1000000 65535 161 255 1 7986
```

- **Decimal, hexadecimal (`0x`) and binary (`0b`).** There is no octal: `017` is
  seventeen. The prefixes are lowercase; `0XFF` is an error.
- **`_` separates digits** anywhere between them, but can't start or end a number, or
  follow the prefix: `1_`, `0x_1` and `_1` aren't numbers (`_1` is a name).
- **A suffix gives the type**: `i8`, `i16`, `i32`, `i64`, `i128`, `u8`, `u16`, `u32`, `u64`,
  `u128`, as in `255u8` and `-1i64`.
- **Suffixes never clash with hex digits**: `i` and `u` aren't hex digits, and the float
  suffixes apply only to decimal literals, so `0x1f32` is the integer `0x1F32`, 7986.
- **C's `L` and `U` suffixes aren't there**: `1L` is an error that lists the suffixes.

### Floats

A float literal has a digit on both sides of its `.`, an exponent, or a float suffix:
`0.5`, `2.5e3`, `1e-3`, `1.5f32`, `2f32`, `3f64`. `.5` and `5.` aren't literals, and `1.hex()`
is a method called on the integer 1.

### Characters

A character literal is an integer: the code point between the quotes. `'a'` is 97, `'\n'` is
10, and `'\u{1F600}'` is 0x1F600. There is no `char` type: a code point is a `u32`, and a
byte of text is a `u8` ([DESIGN Appendix A.10](../DESIGN.md#a10-a-char-type)).

```java
void main() {
    var c = 'a';
    u8 z = 'z';
    u32 smile = '\u{1F600}';
    println("${c} ${z} ${smile} ${'\n'} ${'\''}");
}
```

```output
97 122 128512 10 39
```

- Like other integer literals, it takes its type from where it's used, and is a `u32` with
  nothing to go by.
- **Used as a `u8`, it must be ASCII.** `é` is code point 233, which fits in a byte, but in
  UTF-8 text it is two bytes, so a comparison of one byte with `'é'` would never match:

```java
void main() {
    u8 c = 'é';
}
```

```error
example.zn:2:12: error: this character isn't ASCII, so it can't be a `u8`
```

- It must fit its type: `u16 c = '\u{1F600}';` is an error.
- The escapes are the string escapes; [Text](03-text.md) lists them.

### Strings, `true`, `false` and `null`

A string literal is text in double quotes, on one line: `"hello, ${name}\n"`. Its type is
`string`, a read-only view of bytes, and `${...}` puts a value into it.
[Text](03-text.md) covers strings, escapes, interpolation and where the text it builds is
kept.

`true` and `false` are the two `bool` values. `null` is the value of a nullable reference,
`T&?`, or a nullable function, and of nothing else: `i64 x = null;` is an error
([Memory and references](05-memory.md)).

## Number types

| Type | Size in bytes | Values |
|---|---|---|
| `i8` `i16` `i32` `i64` `i128` | 1, 2, 4, 8, 16 | signed integers |
| `u8` `u16` `u32` `u64` `u128` | 1, 2, 4, 8, 16 | unsigned integers |
| `f32` `f64` | 4, 8 | IEEE 754 floating point |
| `bool` | 1 | `true` and `false` |

- **One spelling per type.** There are no aliases, so `long`, `short`, `byte`, `double`
  and `int` are errors that suggest the name to use.
- **Lengths, indexes and cursor positions are `i64`.** Indexes are signed, so an index
  computed below 0 is out of range instead of wrapping to a huge one. `xs.len` is an `i64`,
  so `u32 n = xs.len;` needs a cast.
- **`i128` and `u128`** are for what 64 bits can't hold: the full product of two `u64`s,
  128-bit hashes and IDs, IPv6 addresses, sums that would overflow. They follow every rule
  below, and map to the C compilers' `__int128`.
- **`bool` isn't a number.** There's no arithmetic on it and no cast to or from it, though
  `&`, `|` and `^` work on two `bool`s, and `false < true`.

The full reasons are in [DESIGN §9.1](../DESIGN.md#91-number-types-and-sizes-decided).

## How a literal gets its type

A literal takes its type from where it's used. With nothing to go by, an integer literal is
an `i64`, the type of lengths and indexes; a float literal is an `f64`; and a character
literal is a `u32`.

```java
void main() {
    u8 b = 200;
    f64 x = 1;
    var i = 0;
    var big = 100000 * 100000;
    u32 c = 7;
    var d = c - 8;
    println("${b} ${x} ${i} ${big} ${d}");
    println("${1 << 40} ${max(3, 5)} ${'a'} ${7 / 2} ${7.0 / 2}");
}
```

```output
200 1.0 0 10000000000 4294967295
1099511627776 5 97 3 3.5
```

- **`u8 b = 200;`** makes the `200` a `u8`, and `f64 x = 1;` makes the `1` an `f64`.
- **`var i = 0;`** has nothing to go by, so `i` is an `i64`. So are `1 << 40` and
  `max(3, 5)`, and `7 / 2` is integer division.
- **The type flows into arithmetic.** In `100000 * 100000` both literals are `i64`, so the
  product is computed in 64 bits. In `c - 8`, the `8` takes the type of `c`, a `u32`, and
  the subtraction wraps (below, "Arithmetic"). In `7.0 / 2`, the `2` is an `f64`.
- **A literal must fit its type**, which is checked when the program is compiled, constant
  arithmetic included:

```java
void main() {
    u8 b = 256;
    i32 big = 100000 * 100000;
}
```

```error
example.zn:2:12: error: 256 doesn't fit in u8
example.zn:3:15: error: 10000000000 doesn't fit in i32
help: `(i32%) x` wraps and `(i32|) x` saturates, if that's what you want (DESIGN §9.1)
```

- **A signed type's minimum can be written after `-`**: `i8 lo = -128;` and
  `i64 m = -9223372036854775808;` compile, as in Java, though `128` alone doesn't fit an
  `i8`, and so does `-128i8` with a suffix.
- **A literal with nothing to give it a type is an `i64`**, so `var m =
  9223372036854775808;` is an error that says to write the type: `u64 m =
  9223372036854775808;`.
- **A literal above `i128.MAX` is a `u128`**, the only type that holds it:
  `var all = 0xffff_ffff_ffff_ffff_ffff_ffff_ffff_ffff;` is `u128.MAX`.

## Implicit conversions

A value converts to another type by itself only when the other type holds every value of
its own:

- a wider integer of the same signedness: `i8` to `i32`, `u16` to `u64`;
- a wider signed integer, for an unsigned one: `u8` to `i16`, `u32` to `i64`;
- `f32`, for integers of up to 16 bits;
- `f64`, for integers of up to 32 bits, and for `f32`.

Everything else is a cast (below), and the compiler says which one to write:

```java
void main() {
    u32 c = 7;
    i64 len = 10;
    u8 b = c;
    f64 x = len;
}
```

```error
example.zn:4:12: error: u32 doesn't convert to u8 implicitly
help: write `(u8) x`, which stops the program if the value doesn't fit (DESIGN §9.1)
example.zn:5:13: error: i64 doesn't convert to f64 implicitly
```

An `i64` doesn't become an `f64` by itself because an `f64` holds integers exactly only up
to 2⁵³. An array also converts to a slice of its elements, wherever a slice is expected
([Arrays](04-arrays.md)).

## Mixing integer types

An arithmetic or bitwise operator on two different integer types works when one type holds
every value of the other, and the result has that type. Otherwise it's an error, and a cast
says what's meant. A third, wider type is never chosen.

```java
void main() {
    u8 b = 200;
    i32 n = -7;
    u32 c = 4000000000;
    i64 len = 10;
    f64 x = 0.5;
    var sum = n + b;
    var diff = len - c;
    var scaled = n * x;
    println("${sum} ${diff} ${scaled}");
}
```

```output
193 -3999999990 -3.5
```

- `n + b` is an `i32`, since every `u8` fits in an `i32`; `len - c` is an `i64`.
- **Integers with floats follow the same rule**: `n * x` with an `i32` `n` is an `f64`,
  while an `i64` needs `(f64) len`.
- **Literals take the other operand's type**, so `c - 1` and `b + 1` need nothing.
- **`x += y` needs `y` to convert to the type of `x`.** With an `i32` `total`,
  `total += len` is an error.

```java
void main() {
    u32 c = 7;
    i32 d = -1;
    u64 h = 99;
    i64 len = 10;
    var a = c + d;
    var b = h % len;
}
```

```error
example.zn:6:13: error: neither u32 nor i32 holds every value of the other
help: cast one side, as in `(i64) x` (DESIGN §9.2)
example.zn:7:13: error: neither u64 nor i64 holds every value of the other
```

So a result always has one of its operands' types, and a `u64` with an `i64` gets the same
treatment as a `u32` with an `i32`, though an `i128` would hold both
([DESIGN §9.2](../DESIGN.md#92-integer-arithmetic)).

## Comparisons

`==`, `!=`, `<`, `<=`, `>` and `>=` between two integer types are exact, whatever the types:

```java
void main() {
    i32 d = -1;
    u32 c = 0;
    i64 neg = -1;
    u64 top = u64.MAX;
    println("${d < c} ${d == c} ${neg == top} ${neg < top}");
}
```

```output
true false false true
```

In C, `d < c` is false, because `-1` becomes `UINT_MAX`. When neither type holds the other,
as here, the comparison costs one extra test, as C++20's `std::cmp_less` does.

- **An integer and a float** compare under the mixing rule above: an `i32` with an `f64`
  works, and an `i64` with an `f64` needs a cast.
- **Floats compare as IEEE 754 says**: every comparison with NaN is false, except `!=`,
  which is true. See "Floats" below.
- **Strings compare byte by byte**, with `==` and `<` ([Text](03-text.md)).
- **`==` on two class values compares their fields** in the design; the C backend doesn't
  support it yet ([What isn't there yet](15-not-yet.md)).

## Arithmetic

| Operators | On integers | Also on |
|---|---|---|
| `+` `-` `*`, unary `-`, `++` `--` | wrap on overflow, modulo 2^width | floats, as IEEE 754 says |
| `+#` `-#` `*#` | fail with `RANGE` on overflow | |
| `+\|` `-\|` `*\|` | saturate at the type's `MIN` or `MAX` | |
| `/` `%` | fail with `DOMAIN` when dividing by zero | `/` on floats, which never fails |
| `&` `\|` `^` `~` `<<` `>>` | never fail | `&` `\|` `^` on two `bool`s |

Each binary one has a compound form: `+=`, `+#=`, `+|=`, `<<=` and the rest.

### Wrapping, checking and saturating

```java
void main() {
    i64 big = i64.MAX;
    u8 b = 250;
    u32 zero = 0;
    println("${big + 1} ${b + 10} ${zero - 1} ${-i64.MIN}");
    u8 c @err = b +# 10;
    println("${c} ${status.name(err)}");
    println("${b +| 10} ${b -| 251} ${b *| 2}");
    i8 s = -100;
    println("${s -| 100} ${s +| 100}");
}
```

```output
-9223372036854775808 4 4294967295 -9223372036854775808
0 RANGE
255 0 255
-128 0
```

- **`+`, `-` and `*` wrap**, for signed and unsigned types alike, in every build:
  `i64.MAX + 1` is `i64.MIN`, and `-i64.MIN` is `i64.MIN`. So do `++`, `--` and the
  compound forms. Java, Go and C# wrap the same way.
- **`+#`, `-#` and `*#` fail with `RANGE`** on overflow. `#` is the status symbol, so
  `a +# b` reads as an add that can give a status. `@err` reads it, and the result is then
  0; a failure that nothing reads stops the program, at its line, with exit status 1:

```java
void main() {
    i64 n = i64.MAX;
    println("before");
    i64 m = n +# 1;
    println("after ${m}");
}
```

```output
before
```

```stderr exit=1
example.zn:4: integer overflow
```

- **`+|`, `-|` and `*|` saturate**: the result is the nearest value the type holds.
- **`+#=` that fails leaves its variable as it was.**
- `+#` and `+|` take integers only: `a +# 2.0` is an error.
- `+%`, `-%` and `*%` were the wrapping operators before `+` wrapped. They are errors now
  that name `+` and `+#`.

Why wrapping is the default: a check on every operation cost more than anything else
measured against Goose, C++ and Rust, from 1.5 to 2.5 times the run time in loops such as
Levenshtein distance, and no compiler can prove such checks away when overflow depends on
the data. Wrapping can't break memory safety, since indexes are still checked, and a cast
that doesn't fit still fails, which is where silent truncation does the most harm. Code
that wants an overflow caught writes `+#`, as the library does where it promises a failure,
in `sum()` and `abs` ([DESIGN §9.2](../DESIGN.md#92-integer-arithmetic)).

### Division and remainder

```java
void main() {
    i64 zero = 0;
    println("${-7 / 2} ${-7 % 2} ${7 % -2} ${i64.MIN / -1} ${i64.MIN % -1}");
    i64 q @why = 7 / zero;
    println("${q} ${status.name(why)} ${7.divOr(zero, -1)}");
}
```

```output
-3 -1 1 -9223372036854775808 0
0 DOMAIN -1
```

- **Integer division truncates toward zero**, and `%` has the sign of the dividend, as in
  C and Java.
- **`x / 0` and `x % 0` fail with `DOMAIN`.** When nothing reads that, the program stops
  with a message such as `tool.zn:12: division by zero`.
- **`MIN / -1` wraps to `MIN`**, as `-MIN` does, and `MIN % -1` is 0.
- **`x.divOr(d, fallback)`** gives `fallback` instead of dividing by zero.
- **On floats, `/` never fails**: `1.0 / 0.0` is infinity. `%` takes integers only.

### Shifts and bitwise operators

```java
void main() {
    i64 neg = -16;
    u64 top = 0xF000_0000_0000_0000;
    u8 b = 0b1100_1010;
    println("${neg >> 2} ${top >> 60} ${1 << 40}");
    println("${b & 0x0F} ${b | 1} ${b ^ 0xFF} ${~b} ${b << 3}");
    i64 far = 70;
    println("${1 << far} ${neg >> far}");
}
```

```output
-4 15 1099511627776
10 203 53 53 80
0 -1
```

- **`>>` is an arithmetic shift on signed types and a logical one on unsigned types**, so
  Java's `>>>` isn't needed, and isn't there.
- **The result has the left operand's type**, and the count can be any integer type.
  `b << 3` on a `u8` keeps 8 bits.
- **Shifts never fail.** A count below 0, or of the width or more, shifts every bit out:
  the result is 0, or -1 for a negative value shifted right.
- **`&`, `|` and `^` follow the mixing rule**, as arithmetic does, and also work on two
  `bool`s, without short-circuiting.

## Casts

A cast converts a number to another number type. It never changes a value silently: the
plain cast stops the program when the value doesn't fit, and two marks choose another
outcome.

```java
void main() {
    i64 n = 300;
    i64 m = -5;
    println("${(u8%) n} ${(u8%) m} ${(u8|) n} ${(u8|) m}");
    u8 b @bad = u8.from(n);
    println("${b} ${status.name(bad)}");
    i64 x = -1;
    println("${(u64%) x} ${(u8) (n & 0xFF)}");
    f64 f = 3.7;
    println("${(i64) f} ${(i64) -f} ${(u8|) 300.0} ${(u8|) f64.NAN}");
    println("${(f64) i64.MAX} ${(f32) 1e300}");
}
```

```output
44 251 255 0
0 RANGE
18446744073709551615 44
3 -3 255 0
9223372036854775808.0 Infinity
```

| Written | When the value doesn't fit |
|---|---|
| `(u8) n` | stops the program, as with `n` = 300 or -5 |
| `(u8%) n` | wraps, keeping the low 8 bits: 300 gives 44, and -5 gives 251 |
| `(u8\|) n` | saturates: 300 gives 255, and -5 gives 0 |
| `u8.from(n)` | fails with `RANGE` and gives 0, for `@` to read |

- **The marks are the ones the operators use**: `%` for wrapping, as the old `+%` did, and
  `|` for saturating, as in `+|`.
- **`u8.from(n)` is for values from outside the program**, such as a port number read from
  a configuration file, where a bad value is an error to report, not a bug.
- **A plain cast that doesn't fit stops the program**:

```java
void main() {
    i64 n = 300;
    u8 b = (u8) n;
    println("${b}");
}
```

```stderr exit=1
example.zn:3: the value doesn't fit in u8
```

- **The check is one compare and branch**, which the C compiler removes at `-O2` when it
  can prove the value fits, as in `(u8) (n & 0xFF)`.
- **A constant that doesn't fit is a compile error**: `(u8) 300` doesn't compile, while
  `(u8%) 300` is 44.
- **Remainders keep their sign**: `(u8) (i % 256)` stops when `i` is negative, since
  `-1 % 256` is -1. `(u8%) i` takes the low byte of any `i`.
- **Between signed and unsigned types of the same size**, only `%` reinterprets the bits:
  with `x` = -1, `(u64%) x` is `u64.MAX`, and `(u64) x` stops.
- **Float to integer truncates toward zero.** NaN, or a value out of range, stops the
  program; `(u8|) x` saturates, with NaN giving 0; and `%` is an error. To round instead,
  use `math.round` first, or `math.roundToI64`.
- **To a float, nothing stops.** An integer rounds to the nearest value, and `(f32) x`
  rounds to the nearest `f32`, or to an infinity beyond its range. `%` and `|` are errors
  with a float target.
- **Casts are only for numbers.** `(bool) n` suggests `n != 0`, and a `bool` to a number
  suggests `b ? 1 : 0`. Interface references use `instanceof`
  ([Classes, interfaces and generics](06-classes.md)).

```java
void main() {
    f64 x = 2.5;
    i64 n = 3;
    var a = (u8) 300;
    var b = (u8%) x;
    var c = (f64|) n;
    var d = (bool) n;
}
```

```error
example.zn:4:13: error: 300 doesn't fit in u8
example.zn:5:13: error: a float can't wrap to an integer
example.zn:6:13: error: `%` and `|` don't apply to a float target
example.zn:7:13: error: there's no cast to bool
```

## Floats

`f64` is the default float type; `f32` takes a suffix (`1.5f32`) or a declared type. Both are
IEEE 754, and arithmetic on them never fails.

```java
void main() {
    f64 zero = 0.0;
    f64 inf = 1.0 / zero;
    f64 nan = zero / zero;
    println("${inf} ${-inf} ${nan} ${f64.EPSILON}");
    println("${nan == nan} ${nan != nan} ${nan < 1.0} ${nan.isNaN()} ${inf.isFinite()}");
    println("${0.1 + 0.2} ${1.0 / 3.0} ${100.0} ${1e20} ${1e21} ${1.5e-7} ${0.000001}");
    f32 third = 1.0f32 / 3.0f32;
    println("${third} ${(f64) third} ${-0.0}");
}
```

```output
Infinity -Infinity NaN 2.220446049250313e-16
false true false true false
0.30000000000000004 0.3333333333333333 100.0 100000000000000000000.0 1e21 1.5e-7 0.000001
0.33333334 0.3333333432674408 -0.0
```

- **Dividing by zero gives an infinity**, or NaN for `0.0 / 0.0`, as IEEE 754 says.
- **Every comparison with NaN is false, except `!=`**, so `nan == nan` is false. Test with
  `x.isNaN()`. Sorting and the binary searches use a total order instead, which puts NaN
  last, so a NaN can't break a sort ([Arrays](04-arrays.md)).
- **`${x}` prints the shortest text that parses back to the same value**: `0.1` prints as
  `0.1`, and `0.1 + 0.2` shows why it isn't `0.3`. A whole number keeps a `.0`, so a float
  never prints like an integer.
- **Plain digits from 10⁻⁶ to below 10²¹, and an exponent outside that**, as in
  JavaScript: `1e21`, `1.5e-7`. Below 10²¹, a large whole value prints every digit of the double it is, so
  `123456789012345680000.0` prints as `123456789012345683968.0`.
- **Infinities and NaN print as `Infinity`, `-Infinity` and `NaN`.** Zero keeps its sign.
- **An `f32` prints its own shortest text**, `0.33333334`, not that of the `f64` it widens
  to.
- **`f32` and `f64` have `NAN`, `INFINITY` and `EPSILON`**, and no `MIN` or `MAX`.
- **Multiplies and adds aren't fused**, so a result is the same however the program is
  compiled ([Getting started](01-getting-started.md#the-c-compiler)).
- `%`, the bitwise operators, `+#` and `+|` take integers only.
- Formatting a float with a fixed number of digits, `x.fixed(2)`, and the other formatting
  methods are in [Text](03-text.md).

## Methods and functions on numbers

### Parsing

`T.parse(s)` reads a number of type `T` from text. It fails with `INVALID` when the text
isn't one, or, for integers, `RANGE` when it doesn't fit, and gives 0 then.

```java
void main() {
    i64 a @e1 = i64.parse("-42");
    i64 b @e2 = i64.parse("12x");
    u8 c @e3 = u8.parse("300");
    i64 h @e4 = i64.parse("ff", 16);
    f64 f @e5 = f64.parse("2.5e3");
    println("${a} ${e1} | ${b} ${status.name(e2)} | ${c} ${status.name(e3)} | ${h} ${e4} | ${f} ${e5}");
    (i64 n, string rest) @e6 = i64.parsePrefix("42px");
    println("${n} [${rest}] ${e6}");
}
```

```output
-42 0 | 0 INVALID | 0 RANGE | 255 0 | 2500.0 0
42 [px] 0
```

- **`parse` accepts a sign, then digits**, with no spaces, no `_` and no `0x`. Unsigned
  types take no `-`: `u8.parse("-1")` is `INVALID`.
- **`T.parse(s, base)`**, on integer types, reads digits in a base from 2 to 36, with no
  prefix.
- **`f64.parse` and `f32.parse`** also take a fraction, an exponent, `inf` and `nan`. A
  float never fails with `RANGE`: a value beyond the type's range parses as an infinity,
  and one too small as 0. They round correctly, so printing and parsing a float gives the
  same float back.
- **`T.parsePrefix(s)`** reads the number at the start of `s` and gives the rest of `s`
  too, for tokenizers. It gives two results ([Functions and lambdas](08-functions.md)).
- An unread failure stops the program, so `i64 n = i64.parse(s);` is for text the program
  trusts. [Errors and statuses](07-errors.md) covers `@`.

### Limits

`T.MIN` and `T.MAX` are on every integer type: `i8.MIN` is -128, `u64.MAX` is
18446744073709551615, `u128.MAX` has 39 digits. The float types have `NAN`, `INFINITY` and
`EPSILON`.

### `min`, `max`, `clamp` and `abs`

```java
void main() {
    println("${min(3, 9)} ${max(-1, 2)} ${max(2.5, 1.0)} ${clamp(15, 0, 10)} ${abs(-7)} ${abs(-2.5)}");
    i64 c @bad = clamp(5, 10, 0);
    println("${c} ${status.name(bad)}");
    println("${i8.MIN} ${i8.MAX} ${u16.MAX}");
}
```

```output
3 2 2.5 10 7 2.5
0 INVALID
-128 127 65535
```

- **They are generic functions**, available everywhere: `min`, `max` and `clamp` take any
  ordered type, and `abs` any number type.
- **Both arguments have one type.** No conversion is applied to make two types agree:

```java
void main() {
    u8 small = 3;
    i32 wide = 4;
    var m = max(small, wide);
}
```

```error
example.zn:4:13: error: `T` would be both u8 and i32 here; no conversion is applied to make them agree (DESIGN §10)
```

- **`clamp(x, lo, hi)` fails with `INVALID`** when `lo > hi`, and gives 0.
- **NaN counts as the largest float** in `min` and `max`, as in sorting, whatever the order
  of the arguments: `min(f64.NAN, 1.0)` is 1.0, and `max(1.0, f64.NAN)` is NaN
  ([DESIGN §10.1](../DESIGN.md#101-built-in-bounds-decided)).
- **`abs` of a type's minimum overflows**, which stops the program: `abs(i64.MIN)`.
- `x.compareTo(y)` gives -1, 0 or 1, as an `i32`.

### Bit operations

These are methods on every integer type:

```java
void main() {
    u32 bits = 0b1011_0000;
    println("${bits.popCount()} ${bits.leadingZeros()} ${bits.trailingZeros()} ${bits.isPowerOf2()} ${64u32.isPowerOf2()}");
    u8 r = 0b1000_0001;
    println("${r.rotateLeft(1)} ${r.rotateRight(1)} ${0x1234u16.reverseBytes().hex()}");
}
```

```output
3 24 4 false true
3 192 3412
```

- `popCount()`, `leadingZeros()` and `trailingZeros()` give an `i64`; the zeros are counted
  in the value's own width, so a `u32` has 24 leading zeros here.
- `rotateLeft(n)`, `rotateRight(n)` and `reverseBytes()` give the value's own type.
- `isPowerOf2()` gives a `bool`.
- `hex()`, `bin()` and `oct()` print an integer in base 16, 2 and 8 ([Text](03-text.md)).

### `math`

`math` holds the float functions and constants, in every file; `import zink.math.*;` is
accepted, and not needed.

```java
void main() {
    println("${math.sqrt(2.0)} ${math.pow(2.0, 10.0)} ${math.hypot(3.0, 4.0)}");
    println("${math.floor(-2.5)} ${math.ceil(-2.5)} ${math.trunc(-2.5)} ${math.round(-2.5)} ${math.round(2.5)}");
    println("${math.PI} ${math.log2(8.0)} ${math.atan2(1.0, 1.0)} ${math.sqrt(-1.0)}");
    f32 x = 2.0f32;
    println("${math.sqrt(x)}");
    i64 n @bad = math.roundToI64(f64.NAN);
    println("${math.roundToI64(2.5)} ${n} ${status.name(bad)}");
}
```

```output
1.4142135623730951 1024.0 5.0
-3.0 -2.0 -2.0 -3.0 3.0
3.141592653589793 3.0 0.7853981633974483 NaN
1.4142135
3 0 RANGE
```

- **The float functions take an `f32` or an `f64`** and give the same type: `sqrt`, `cbrt`,
  `exp`, `log`, `log2`, `log10`, `pow`, `hypot`, `sin`, `cos`, `tan`, `asin`, `acos`,
  `atan`, `atan2`, `sinh`, `cosh`, `tanh`, `floor`, `ceil`, `trunc` and `round`. They are
  the C library's.
- **They follow IEEE 754 and never stop the program**: `math.sqrt(-1.0)` is NaN.
- **`math.round` rounds half away from zero.**
- **`math.roundToI64(x)`** rounds, and fails with `RANGE` for NaN, infinities and values an
  `i64` can't hold.
- **`math.PI`, `math.TAU` and `math.E`** are `f64`s.
- Not yet: the integer functions `math.gcd`, `math.lcm`, `math.ipow` and `math.isqrt` pass
  `zink check`, and the C backend reports them as not supported yet
  ([What isn't there yet](15-not-yet.md)).

## `bool` and conditions

A condition, in `if`, `while`, `do`, the condition of `for` and `?:`, and an operand of `!`,
`&&` and `||`, is a `bool` or an unsigned integer. An unsigned integer counts as false when
it is 0 and true otherwise, so `if (err)` tests a status.

```java
void main() {
    u8 err = 2;
    u64 mask = 0;
    if (err) {
        println("failed");
    }
    if (!mask) {
        println("no bits set");
    }
    bool both = err && !mask;
    println("${both} ${err ? "yes" : "no"} ${true & false} ${true ^ true} ${false < true}");
}
```

```output
failed
no bits set
true yes false false true
```

- **Signed integers and floats aren't conditions**: write `n != 0`.
- **There's no conversion outside conditions**: `bool b = err;` is an error; write
  `err != 0`.
- **An assignment can't be a condition**: `if (x = 0)` is an error, since with integers as
  conditions it is usually a typo for `==`. An assignment inside a comparison,
  `if ((n = next()) > 0)`, is fine.
- **`&&` and `||` short-circuit**; `&` and `|` on `bool`s don't.

```java
void main() {
    i64 n = 1;
    u8 err = 0;
    if (n) {
        println("n");
    }
    bool b = err;
    if (err = 0) {
        println("zero");
    }
}
```

```error
example.zn:4:9: error: a signed integer (i64) isn't a condition
help: write `n != 0` (DESIGN §9.4)
example.zn:7:14: error: expected bool, found u8
example.zn:8:9: error: an assignment can't be a condition
help: to compare, write `==` (DESIGN §9.4)
```

The reasons are in [DESIGN §9.4](../DESIGN.md#94-statements-and-expressions-decided).

## Variables

A local variable is declared with its type, or with `var`, which takes the type of its
initializer:

```java
void main(string[.] args) {
    var count = 0;
    var ratio = 0.5;
    var name = "zinc";
    var small = 3u8;
    i64 a = 1, b = 2;
    i64 total;
    if (args.len > 0) {
        total = 1;
    } else {
        total = 2;
    }
    println("${count} ${ratio} ${name} ${small} ${a + b} ${total}");
}
```

```output
0 0.5 zinc 3 3 2
```

- **`var` needs an initializer** to take its type from, and can't take `null`, which has no
  type of its own. `var count = 0;` is an `i64`.
- **`var` copies**, as `=` does everywhere; `var& r = items[0];` binds a reference instead.
  Copies and references are the subject of [Memory and references](05-memory.md).
- **A variable must be assigned before it is read, on every path.** The compiler checks
  this, so a variable declared without a value is fine as long as each branch gives it one:

```java
void main(string[.] args) {
    i64 total;
    if (args.len > 0) {
        total = 1;
    }
    println("${total}");
}
```

```error
example.zn:6:16: error: `total` may be read before it is assigned: some paths to here don't assign it
help: assign it on every path before this, as with an `else` (DESIGN §6.2)
```

- **A variable lasts to the end of its block.** A name can't be declared twice in one
  block, and a nested block may declare it again, hiding the outer one until it ends.
- **Top-level fields of a script file** are static variables, which every function of the
  file can read and change, and which are set before `main` runs
  ([Programs, packages and building](10-programs.md)).
- **There are no `final` locals**: `final` goes on fields ([`final`
  fields](06-classes.md#final-fields)), and on a local it is an error. The compiler already
  knows which locals change, and a lambda's captures are checked without it
  ([Functions and lambdas](08-functions.md)).
- **A named constant is a `static final`**, `static final i64 LIMIT = 64;`
  ([Constants](06-classes.md#constants)).

## Operators and precedence

Operators follow Java's precedence. From the loosest to the tightest:

| Level | Operators | Associativity |
|---|---|---|
| 1 | lambda `=>` | right |
| 2 | `=` `.=` and the compound assignments, status captures `x @err = ...` | right |
| 3 | `? :` | right |
| 4 | `\|\|` | left |
| 5 | `&&` | left |
| 6 | `\|` | left |
| 7 | `^` | left |
| 8 | `&` | left |
| 9 | `==` `!=` | left |
| 10 | `<` `>` `<=` `>=` `instanceof` | left |
| 11 | array inserts `+>` `+<` `+^` | left |
| 12 | `<<` `>>` | left |
| 13 | `+` `-` `+#` `-#` `+\|` `-\|` | left |
| 14 | `*` `/` `%` `*#` `*\|` | left |
| 15 | prefix `+` `-` `!` `~` `++` `--`, status read `@`, casts `(i32) x` | right |
| 16 | postfix `++` `--`, array removes `->` `-<` `-^` `^-`, `.`, calls, indexing | left |

[GRAMMAR §6](../GRAMMAR.md#6-expressions) has the full grammar, with how the parser tells
a cast, a lambda and a parenthesized expression apart.

- **The bitwise operators bind more loosely than `==`**, as in Java and C, so
  `x & 1 == 0` means `x & (1 == 0)`, which is an error. Write `(x & 1) == 0`:

```java
void main() {
    u8 x = 6;
    println("${x & 1 == 0}");
}
```

```error
example.zn:3:20: error: `&` needs integers or two bools, and this is bool
```

- **The lexer takes the longest match.** `a^-b` lexes as `a ^- b`, the array operator that
  removes after a cursor, so an XOR with a negated value needs a space: `a ^ -b`. The
  error says so. `i-->0` lexes as `i -- > 0`, as in Java.
- **`++` and `--` give a value**: `i++` the old one, `++i` the new one.
- **Assignments give a value too**, so `a = b = 3;` assigns both.
- **There is no `+` on strings**: text is built with `${...}` ([Text](03-text.md)).
- **An expression statement has to do something**: an assignment, `++` or `--`, a call, or
  an array operator. `x + 1;` is an error that says its value isn't used.

## Statements

### `if` and `else`

```java
void main() {
    i64 n = 27;
    i64 steps = 0;
    while (n != 1) {
        if (n % 2 == 0) {
            n /= 2;
        } else {
            n = 3 * n + 1;
        }
        steps++;
    }
    println("${steps}");
    string size = steps > 100 ? "long" : "short";
    println(size);
}
```

```output
111
long
```

- The condition is in parentheses, and follows the rules above.
- **The body is any statement**, as in Java, so braces aren't required; this documentation
  always writes them. `else if` is an `else` whose statement is an `if`.
- `c ? a : b` is the expression form.

### `while` and `do`

`while (cond) { ... }` tests before each pass; `do { ... } while (cond);` tests after, so its
body runs at least once.

```java
void main() {
    i64 k = 0;
    do {
        k++;
    } while (k < 0);
    println("${k}");
    while (true) {
        k *= 2;
        if (k > 50) {
            break;
        }
    }
    println("${k}");
}
```

```output
1
64
```

A second form, `while (T x : call)`, makes the call before each pass and runs while it
succeeds: the *repeat loop*, as in `while (i64 x : stack ->)`. It is covered with statuses in
[Errors and statuses](07-errors.md#repeat-loops), and with iterators in
[Maps, sets, pools and iterators](09-collections.md#the-repeat-loop).

### `for`

The classic `for` is Java's: `for (init; condition; step)`, any part of which can be left
out.

```java
void main() {
    for (var i = 0, j = 10; i < j; i += 3, j--) {
        print("${i}:${j} ");
    }
    println("");
    i64 x = 0;
    for (; x < 3;) {
        x++;
    }
    for (;;) {
        break;
    }
    println("${x}");
}
```

```output
0:10 3:9 6:8 
3
```

- `for (var i = 0; ...)` makes `i` an `i64`, the type of an integer literal with nothing to
  go by, and the type of indexes.
- The init can declare several variables of one type, and the step can be a list.
- The variables it declares last until the loop ends, so two loops can each declare `i`.
- A loop over an array's indexes runs without index checks when a test before the loop
  shows every index in range ([DESIGN §14.2](../DESIGN.md#142-what-emitting-c-requires)).

### For-each

`for (T x : xs)` visits each element of an array, a slice, a map, a set, a pool or an
iterator, in order:

```java
void main() {
    i64[..>] xs = [1, 2, 3];
    for (i64 x : xs) {
        print("${x} ");
    }
    println("");
    for (i64& x : xs) {
        x *= 10;
    }
    println("${xs}");
    for (u8 b : "hi") {
        print("${b} ");
    }
    println("");
}
```

```output
1 2 3 
[10, 20, 30]
104 105 
```

- **`for (i64 x : xs)` copies each element**, and `for (i64& x : xs)` refers to it in
  place, so the loop can change it. `var` and `var&` work too.
- **A `string` gives its bytes**, as `u8`s. `s.codePoints()` gives code points, and
  `s.lines()` lines, one at a time ([Text](03-text.md)).
- Arrays are [Arrays](04-arrays.md); maps, sets, pools and iterators are
  [Maps, sets, pools and iterators](09-collections.md).

### `switch`

`switch` has only the arrow form: each arm is `case` with one or more values, or `default`,
then `=>` and an expression or a block. There is no fall-through and no `break`. It works on
integers, characters among them, on `string` and on enums, as a statement or as an
expression.

```java
enum Suit { CLUBS, DIAMONDS, HEARTS, SPADES }

string color(Suit s) {
    return switch (s) {
        case CLUBS, SPADES => "black";
        case DIAMONDS, HEARTS => "red";
    };
}

void main() {
    for (i64 n : [0, 1, 2, 7, -1]) {
        string size = switch (n) {
            case 0 => "none";
            case 1, 2 => "a few";
            case -1 => "negative";
            default => {
                string s = n > 5 ? "many" : "some";
                yield s;
            }
        };
        println("${n}: ${size}");
    }
    switch ("stop") {
        case "go" => println("going");
        case "stop" => {
            println("stopping");
        }
        default => {}
    }
    println(color(Suit.HEARTS));
    u8 c = 'x';
    switch (c) {
        case 'a', 'e', 'i', 'o', 'u' => println("vowel");
        default => println("other");
    }
}
```

```output
0: none
1: a few
2: a few
7: many
-1: negative
stopping
red
other
```

- **A case label is known when the program is compiled**: an integer, possibly negative, a
  character, a string without interpolation, an enum constant, `case HEARTS` or `case
  Suit.HEARTS`, or a constant, `case GRASS` or `case Blocks.GRASS`
  ([Constants](06-classes.md#constants)). A variable, or a static that isn't a constant, is
  an error.
- **A value can't appear in two labels**, however they're written; the second is an error.
- **`yield v;` gives the value of a block arm** of a switch expression. Every path through
  the block must reach a `yield`.
- **A switch expression must cover every case**: with `default`, or with every constant of
  an enum, as in `color`. A switch statement needn't; a value no arm takes does nothing.
- **`switch` works on integers, strings and enums**: not on floats or `bool`.
- `default => {}` is an arm that does nothing.

```java
enum Suit { CLUBS, DIAMONDS, HEARTS, SPADES }

void main() {
    Suit s = Suit.CLUBS;
    string a = switch (s) {
        case CLUBS => "c";
        case HEARTS => "h";
    };
}
```

```error
example.zn:5:16: error: a switch expression must cover every case
help: add a `default` arm, or a case for every enum constant (DESIGN §9.4)
```

Enums are in [Classes, interfaces and generics](06-classes.md#enums).

### `break`, `continue` and labels

`break` leaves the innermost loop and `continue` starts its next pass. A label before a loop
names it, so `break` and `continue` can reach an outer one:

```java
void main() {
    i64[.] grid = [3, 8, 1, 9, 4, 7];
    i64 width = 3;
    i64 foundRow = -1;
    rows: for (i64 r = 0; r < 2; r++) {
        for (i64 c = 0; c < width; c++) {
            if (grid[r * width + c] == 9) {
                foundRow = r;
                break rows;
            }
        }
    }
    println("${foundRow}");
    outer: for (i64 i = 0; i < 3; i++) {
        for (i64 j = 0; j < 3; j++) {
            if (j == 2) {
                continue outer;
            }
            if (i == 2) {
                break outer;
            }
            print("${i}${j} ");
        }
    }
    println("");
}
```

```output
1
00 01 10 11 
```

- **A label names a loop**, and nothing else: a label on a block is an error.
- **`break` is for loops only.** A switch never falls through, so its arms need no
  `break`: one in an arm leaves the loop around the switch, and with no loop it's an error
  that says so.
- `continue` in a `for` runs the step first, as in Java.

### `_` discards

`_` takes a value and throws it away, and can never be read:

```java
i64 second(i64 _, i64 b) {
    return b;
}

void main() {
    _ = second(1, 2);
    i64 n @_ = i64.parse("x");
    i64 count = 0;
    for (i64 _ : [1, 2, 3]) {
        count++;
    }
    println("${n} ${count} ${second(5, 6)}");
}
```

```output
0 3 6
```

- **`_ = e;`** evaluates `e` and discards its value, whatever its type.
- **`x @_ = f();`** discards a status, so a failure neither stops the program nor is read
  ([Errors and statuses](07-errors.md)).
- **`_` can name** a parameter, a lambda parameter, a loop variable or a destructuring
  target that isn't used; several parameters can be named `_`.
- **Reading `_` is an error**, and so is declaring a variable named `_`.
- A call whose result isn't wanted can also be a plain statement: `second(1, 2);`.

## `main`

A program starts at `main`, a top-level function of a script file. It has one of four
forms:

```java no-check
void main() { ... }
void main(string[.] args) { ... }
u8 main() { ... }
u8 main(string[.] args) { ... }
```

```java
u8 main(string[.] args) {
    for (string a : args) {
        println(a);
    }
    return (u8) args.len;
}
```

```
$ zink run show.zn one two
one
two
$ echo $?
2
```

- **`args`** holds the command-line arguments, without the program's name, as a frozen
  array of `string` views. `args.len` counts them.
- **`void main` exits with 0**, and **`u8 main` with what it returns**; falling off the
  end of a `u8 main` returns 0, as in C.
- **The exit status is a `u8`** because Linux keeps only its low 8 bits, and a wider type
  would let `return 256;` reach the shell as success.
- **`main` can't set a status**: `return #1;` is an error that suggests `return 1;`. A
  status is for a caller, and `main` has none.
- **There is no `exit`.** Only returning from `main` ends a program, so a function deep
  in it fails with a status, and its callers pass it up
  ([Errors and statuses](07-errors.md#the-exit-status)).
- **A failure nothing reads stops the program** with exit status 1, and a message on
  standard error that names the line.
- **Any other form is an error.** `i32 main()` is one that says `main` returns `void`, or
  `u8` for the exit status, and so is a parameter of another type:

```java
u8 main(string[..>] args) {
    return 0;
}
```

```error
example.zn:1:4: error: `main` takes no parameters, or `string[.] args` (DESIGN §8.6)
```

A file with no `main` passes `zink check`, which checks a file whether or not it is a whole
program; running or building it is an error, `this program has no `main` to start from`,
and so is a second `main`. The reasons for all of this are in
[DESIGN §8.6](../DESIGN.md#86-script-files-decided).
