# 3. Text

Text in Zinkite is UTF-8 bytes. A `string` is a view of bytes that live somewhere else, and
text a program builds or keeps lives in a `u8` array. This chapter covers both, string and
character literals, interpolation and where the text it makes is kept, printing, the
formatting methods, comparing text, `switch` on strings, the text methods, and UTF-8. The
arrays themselves, their slices and their methods, are in [Arrays](04-arrays.md); why a
view can't outlive its bytes is in [Memory and references](05-memory.md).

## Text is bytes

A `string` is a read-only view of `u8` elements: a pointer and a length, 16 bytes in all.
Its type is `u8[:]`, the slice type of [Arrays](04-arrays.md), under a shorter name.
Indexing gives a byte, `len` counts bytes, a slice of a string is another view of the same
bytes, and a for-each walks the bytes.

```java
void main() {
    string s = "héllo";
    println("${s.len} bytes");
    println("${s[0]} ${s[1]} ${s[2]}");
    println(s[1:3]);
    println(s[^1:]);
    for (u8 b : "hi!") {
        print(" ${b}");
    }
    println("");
}
```

```output
6 bytes
104 195 169
é
o
 104 105 33
```

- **`é` is two bytes in UTF-8**, so `"héllo"` is 6 bytes long and `s[1]` is the first of
  them. Code points come from decoding the bytes (below, "UTF-8 and code points").
- **A byte prints as a number.** `s[0]` is a `u8`, and `${...}` prints integers in decimal.
  To print one byte as text, print a slice of it: `s[0:1]`.
- **A string literal is a view of static data**, which lives as long as the program, so a
  `string` holding one can go anywhere: in a field, an array or a static.
- **A view is read-only.** Writing through one is a compile error:

```java
void main() {
    string s = "abc";
    s[0] = 'x';
}
```

```error
example.zn:3:5: error: can't assign to this: a `string` is a read-only view
```

- A `string` is fixed-size, so an array of strings can be indexed and sorted, and strings can
  be hash map keys ([Maps, sets, pools and iterators](09-collections.md)).
- `main` takes its arguments as `string[.] args` ([The basics](02-basics.md)).

The design is in [DESIGN §4.11](../DESIGN.md#411-strings-decided).

## String literals

A string literal is written in double quotes, on one line. These escapes are recognized:

| Escape | Byte or text |
|---|---|
| `\n` | newline (10) |
| `\t` | tab (9) |
| `\r` | carriage return (13) |
| `\0` | the zero byte |
| `\\` | `\` |
| `\'` | `'` |
| `\"` | `"` |
| `\$` | `$` |
| `\u{...}` | the code point with that hex value, as UTF-8: 1 to 4 bytes |

```java
void main() {
    println("tab:\t| quote: \" backslash: \\ dollar: \$ e-acute: \u{e9} smile: \u{1F600}");
    println("${"a\0b".len} ${"\r\n".len} ${"\u{1F600}".len}");
    println("costs $5, and \${n} is not interpolated");
}
```

```output
tab:	| quote: " backslash: \ dollar: $ e-acute: é smile: 😀
3 2 4
costs $5, and ${n} is not interpolated
```

- **`${` starts an interpolation** (below). A `$` not followed by `{` is an ordinary
  character, so `"$5"` needs no escape; `\$` writes a `$` where a `{` follows.
- **`\u{...}` is a code point**: at most `10FFFF`, and never a surrogate (`D800` to
  `DFFF`).
- **Strings can't span lines.** A line break inside one is an error, `this string isn't
  closed (strings can't span lines)`. Multi-line strings are open (GRAMMAR §8).

Any other escape is an error, and so is a `\u{...}` that isn't a code point:

```java
void main() {
    println("bad \q escape");
}
```

```error
example.zn:2:18: error: `\q` isn't an escape
```

```java
void main() {
    println("\u{D800}");
}
```

```error
example.zn:2:14: error: a Unicode escape must be a code point up to 10FFFF and not a surrogate
```

## Character literals

There is no `char` type. A character literal, in single quotes, is an integer: the value
of its code point. Like any integer literal it takes its type from where it is used, and is a
`u32` when nothing says otherwise. It takes the same escapes as a string.

```java
void main() {
    u8 comma = ',';
    u32 e = 'é';
    var x = 'x';
    println("${comma} ${e} ${x} ${'\n'} ${'\u{1F600}'}");
    string line = "a,b";
    println("${line[1] == ','} ${line[1] == comma}");
}
```

```output
44 233 120 10 128512
true true
```

- **Comparing a byte with a character needs no cast**: in `line[1] == ','`, the `','` is a
  `u8`, the type of the other side.
- **Used as a `u8`, a character must be ASCII.** `'é'` is 233, which fits in a byte, but in
  UTF-8 text `é` is two bytes, so `line[i] == 'é'` could never match. It is an error:

```java
void main() {
    u8 b = 'é';
}
```

```error
example.zn:2:12: error: this character isn't ASCII, so it can't be a `u8`
```

- **A literal must fit its type**, as any integer literal must: `u16 c = '\u{1F600}';` is an
  error, `128512 doesn't fit in u16`.
- **A character literal holds one character.** `'ab'` is an error that says a string uses
  double quotes.
- **`char` is a reserved word**, and using it says that a code point is a `u32`.
- A character literal with nothing to give it a type is a `u32`, so `'Q'.toLower()` looks
  for a method on `u32`, which has none. Write `((u8) 'Q').toLower()`, or give the byte a
  variable of its own.

Why there's no `char` is in [DESIGN Appendix A.10](../DESIGN.md#a10-a-char-type).

## Owned text: `u8` arrays

A view doesn't own its bytes. Text a program reads, builds or keeps is a `u8` array, and
which array says where the bytes live and what can change:

| Type | What it is | Where the bytes live |
|---|---|---|
| `string` (`u8[:]`) | a view: read-only, 16 bytes | in whatever it views |
| `u8[..>]` | a builder: grows at the end | on a data stack |
| `u8[..~]` | a builder that can also shrink at the end, and be popped | on a data stack |
| `u8[.]` | finished text, its length fixed when it is made | where it is declared, built in place |
| `u8[.*]` | finished text with a varint length, for records | inline in the record |
| `u8[..~16]` | up to 16 bytes, editable at the end | inline, in a local or a field |
| `u8[.8]` | exactly 8 bytes | inline |

Every one of them converts to a `string` wherever a `string` is expected: as an argument,
as a method's receiver, in a `string` variable, and in a comparison. So a `u8` array can be
printed, searched and split directly. The view is rooted at the array, and the rules of
[Memory and references](05-memory.md) apply to it.

```java
class User {
    u8[..~16] name;
    i64 age;
}

void main() {
    u8[..>] buf = [];
    buf.append("hello");
    buf +> ',';
    buf.append(" world");
    println(buf);

    u8[.] frozen = "fixed text";
    u8[..~16] small = "bob";
    small.append("by");
    User u = new User(small, 30);
    u8[.5] five = "abcde";
    println("${frozen.len} ${u.name} ${u.name.len} ${five}");

    string view = buf;
    println("${view.len} ${buf.startsWith("hell")} ${small == "bobby"}");
}
```

```output
hello, world
10 bobby 5 abcde
12 true true
```

- **A string literal fills a `u8` array** by copying its bytes in: `u8[.] t = "text";`.
- **A capped array holds at most its cap.** A literal that's longer doesn't compile, and text
  that turns out longer at run time stops the program:

```java
void main() {
    u8[..~4] t = "toolong";
    u8[.5] f = "abc";
}
```

```error
example.zn:2:18: error: this literal has 7 bytes, and the array holds 4
example.zn:3:16: error: an array of type `u8[.5]` holds exactly 5 bytes, and this literal has 3
```

```java
void main() {
    string arg = "a long name";
    u8[..~8] small = arg;
    println(small);
}
```

```stderr exit=1
example.zn:3: the value doesn't fit in an array that holds 8
```

- **A capped array lives inline**, so a class with a `u8[..~16]` field stays fixed-size and
  an array of it can be indexed. A `u8[.*]` or `u8[.]` field makes its class variable-size
  instead, one run of bytes with the text inside it ([Memory and references](05-memory.md)).
- **Assigning a whole array replaces its text**: `b = "reset";`. On a data stack, that is a
  removal under rule R, as `clear()` is.
- **`var copy = buf;` copies.** `var` keeps the array's type, so the copy owns its own bytes;
  `string v = buf;` is a view of `buf` instead.
- **A `u8` array that isn't one block doesn't convert.** A cursor array on a data stack,
  such as `u8[~]`, keeps its elements out of order, so text methods aren't offered on it: the
  error says the type has no such method. `${...}` still prints it as text. Capped ring and
  gap buffers (`u8[~.~8]`, `u8[~16]`) do convert, by first joining their bytes
  ([Arrays](04-arrays.md)).

### A view can't outlive its bytes

A `string` of a buffer is valid only while the buffer holds those bytes. Removing from the
buffer while the view may still be used is a compile error, and so is returning a view of a
local:

```java
void main() {
    u8[..>] buf = [];
    buf.append("hello world");
    string word = buf[0:5];
    buf.clear();
    println(word);
}
```

```error
example.zn:5:5: error: `clear` removes from `buf` while `word` may still refer into it
```

```java
string greeting() {
    u8[..>] buf = [];
    buf.append("hi");
    return buf;
}

void main() {
    println(greeting());
}
```

```error
example.zn:4:5: error: this returns a reference into `buf`, which is released when the function returns
```

A function that makes text returns a `u8[.]`. It is built where the caller keeps it, so it
isn't copied ([DESIGN §3.3](../DESIGN.md#33-building-in-place-decided)):

```java
u8[.] shout(string s) {
    u8[..>] out = [];
    out.append(s.toUpper());
    out +> '!';
    return out;
}

void main() {
    u8[.] a = shout("hey");
    println(a);
    println(shout("you"));
}
```

```output
HEY!
YOU!
```

## Building text

`append(s)` adds text at the end of any `u8` array that can grow there, and `+>` adds one
byte. `+>` returns the array, so it chains.

```java
void money(u8[..>]& out, i64 cents) {
    out.append("$${cents / 100}.${(cents % 100).zeroPad(2)}");
}

void main() {
    u8[..>] line = [];
    line.append("total: ");
    money(line, 1999);
    line +> ' ' +> '(' +> 'U' +> 'S' +> ')';
    println(line);
    line.clear();
    money(line, 5);
    println("${line} ${line.len}");
}
```

```output
total: $19.99 (US)
$0.05 5
```

- **`append` of interpolated text writes the parts straight into the array**, with no
  temporary (below).
- **A `u8[..>]&` parameter takes any `u8` array that grows on a data stack**, a `u8[..~]`
  included. The caller decides how long the text lives, and can reuse the buffer by clearing
  it, as `stdin.nextLine(buf)` does ([The library](13-library-system.md)).
- **`clear()` empties the buffer** in one store, keeping its storage for the next text.

### A capped array can be full

On a capped array, `append` and `+>` fail with `FULL` when the text doesn't fit, and then
store nothing: `append` adds all of its text or none of it. A failure nothing reads stops
the program ([Errors and statuses](07-errors.md)), so code that can fill a capped array reads
the status with `@`.

```java
void main() {
    u8[..~8] t = "abc";
    u8 e1 = @t.append("defgh");
    u8 e2 = @t.append("ij");
    println("${t} ${e1} ${status.name(e2)}");
    u8 e3 = @(t +> '!');
    println("${t} ${status.name(e3)}");
}
```

```output
abcdefgh 0 FULL
abcdefgh FULL
```

```java
void main() {
    u8[..~4] t = "abcd";
    t.append("e");
}
```

```stderr exit=1
example.zn:3: `append` failed with FULL, and nothing read its status
```

- On a `u8[..>]` or `u8[..~]` on a data stack, `append` and `+>` can't fail, so they need no
  `@`.
- **A capped `u8` array passed to a `u8[..>]&` parameter** gets a copy of the function for
  its layout, in which `append` can fail with `FULL` ([Arrays](04-arrays.md)). The
  library's functions whose C fills the buffer, as `stdin.read` and `File.readAll`, take
  only an array on a data stack, and say so where they are called.
- **Not yet:** `appendUtf8(text, from, to)`, which encodes code points from a `u32[:]`,
  type-checks but has no code generated. Append each code point with
  `out.append("${c.utf8()}")` instead (below).

## Interpolation

`${expression}` inside a string literal puts the expression's value into the text. Any
expression works, strings and braces included.

```java
void main() {
    i64 n = 3;
    string who = "world";
    bool ok = true;
    println("${n} ${who} ${ok} ${n * 2 + 1} ${n > 2 ? "big" : "small"} ${"${n}${n}"}");
}
```

```output
3 world true 7 big 33
```

The value's type decides how it prints:

| Value | `${x}` prints |
|---|---|
| `string`, or any `u8` array | the text |
| an integer, `u8` included | the number, in decimal |
| a float | the shortest text that parses back to the same value, with `.0` when it is a whole number: `0.1`, `1.0`, `1.5e300`, `Infinity`, `NaN` |
| `bool` | `true` or `false` |
| any other array | a list: `[1, 2, 3]`, and `[a, b]` for strings, without quotes |
| a class value | a compile error until default formatting is decided |

```java
void main() {
    i64[..>] xs = [1, 2, 3];
    string[..>] words = ["a", "b"];
    u16[..>] codes = [104, 105];
    u8 letter = 'h';
    println("${xs} ${words} ${codes} ${letter} ${0.1} ${1.0} ${1.0 / 3.0} ${1.0 / 0.0}");
}
```

```output
[1, 2, 3] [a, b] [104, 105] 104 0.1 1.0 0.3333333333333333 Infinity
```

```java
class Point {
    i64 x;
    i64 y;
}

void main() {
    Point p = new Point(1, 2);
    println("${p}");
}
```

```error
example.zn:8:16: error: a class value can't be printed until default formatting is decided (DESIGN §17)
```

Print its fields instead: `"(${p.x}, ${p.y})"`. A program's own formatting is a method or
function that returns text, which goes in `${...}` like any other value.

### Where the text goes

Interpolation never builds text it doesn't have to. Where the literal is used decides where
its text is made ([DESIGN §4.11](../DESIGN.md#411-strings-decided)):

- **Printed or appended, the parts are written straight to the output**, one after
  another, with no text built at all. That covers `print`, `println`, `errprint`,
  `errprintln` and `append` to a `u8` array. The design has a `File`'s writes, such as
  `stdout.println(...)`, work the same way; today they take a temporary, as a `string`
  argument does.
- **Declared as a `u8` array, the text is built in the variable's own storage**:
  `u8[.] msg = "${count} files";` writes it once, in `msg`, where it lives as long as `msg`.
- **Anywhere else, it is a temporary**: every part is worked out, then the text is written
  once, at its final size, on a scratch data stack, and released at the end of the
  statement. That is the case for a `string` argument, as in `log("${n} done")`, or for a
  method called on the text.

```java
void log(string msg) {
    println("log: ${msg}");
}

void main() {
    i64 n = 4096;
    log("${n} done");
    u8[.] kept = "${n} kept";
    i64 digits = "${n}".len;
    bool even = "${n}".endsWith("6");
    println("${kept} ${digits} ${even}");
}
```

```output
log: 4096 done
4096 kept 4 true
```

**Keeping a view of a temporary is a compile error**, since the text is gone after the
statement. The message says how to keep it:

```java
void main() {
    i64 x = 3;
    string s = "${x} items";
    println(s);
}
```

```error
example.zn:3:16: error: this text is a temporary, built only for this statement, so `s` can't keep a view of it
help: build it in the variable's own storage instead, as in `u8[.] s = "${x}";` (REFERENCES §3.3)
```

The same goes for a view the temporary passes through a function, and for a method that
returns new text, such as `toUpper()`:

```java
string first(string s) {
    return s[0:1];
}

void main() {
    i64 n = 5;
    string t = first("${n}0");
    println(t);
}
```

```error
example.zn:7:16: error: this text is a temporary, built only for this statement, so `t` can't keep a view of it
```

```java
void main() {
    string w = "abc".toUpper();
    println(w);
}
```

```error
example.zn:2:16: error: this refers into a temporary value, which is released at the end of the statement
```

`u8[.] w = "abc".toUpper();` is the fix in each case: the text is then built in `w`.

- **A failure inside an interpolation is the statement's failure.** `i64.parse` in
  `println("n=${i64.parse(s)}")` stops the program when nothing reads it, like any other
  call; with `u8[.] t @err = "${...}";` the text is empty on failure.

## Printing

```java
void print(string text)          // to stdout
void println(string text)        // adds a newline
void errprint(string text)       // to stderr
void errprintln(string text)
```

- **They take one `string`.** Anything else is printed through interpolation:
  `println("${n}")`. `println(42)` is an error, `expected string, found an integer literal`.
- **They set no status**, so they need no `@`. A write that fails is remembered, reported on
  stderr when the program ends, and makes its exit status 1.
- **`errprint` and `errprintln` write to stderr**, which stays on the terminal when stdout
  is redirected. Use them for error messages.
- **`stdout`, `stderr` and `stdin` are `File`s**, with every method of a file
  ([The library](13-library-system.md)). Their `write`, `print`, `println` and `flush` set a
  status, for code that must check each write. The four functions above are shorthand for
  `stdout.println(x)` and the rest, without the status.

```java
void main() {
    print("one ");
    println("two");
    u8 w = @stdout.write("three\n");
    u8 p = @stdout.println("four ${w}");
    errprintln("to stderr");
    u8 f = @stdout.flush();
    println("statuses ${p} ${f}");
}
```

```output
one two
three
four 0
statuses 0 0
```

```stderr
to stderr
```

**Buffering.** stdout is buffered by lines on a terminal and in 64 KB blocks otherwise,
through C's `stdout`, so C code the program calls ([Calling C](12-calling-c.md)) writes into
the same buffer, in order. stderr is unbuffered, and stdout is flushed before anything goes
to stderr, so the two come out in the order they were written. `stdout.flush()` writes what
is pending, and everything is written out when the program ends.

**A reader that has gone ends the program quietly.** Writing to a pipe whose reader has
closed it ends the program with exit status 141, as a C program killed by `SIGPIPE` would
end, and prints nothing:

```
$ cat count.zn
void main() {
    for (i64 i = 1; i <= 1000000; i++) {
        println("${i}");
    }
}
$ zink build count.zn -o count
$ ./count | head -n 2; echo "${PIPESTATUS[0]}"
1
2
141
$ ./count > /dev/full; echo $?
zink: writing the output failed
1
```

Why printing takes text and formats by method, rather than by format strings, is in
[DESIGN §4.13](../DESIGN.md#413-printing-and-formatting-decided) and
[Appendix A.11](../DESIGN.md#a11-format-specs-and-print-families).

## Formatting numbers

Methods on numbers change how they print. They work anywhere text does, and are most often
written inside `${...}`, where their text goes straight to the output.

| Method | On | Result | Gives |
|---|---|---|---|
| `x.hex()` | integers | `u8[..~N]`, `N` = the type's hex digits (16 for a `u64`) | base 16, lowercase, no prefix |
| `x.oct()` | integers | `u8[..~N]` | base 8 |
| `x.bin()` | integers | `u8[..~N]`, 64 for a `u64` | base 2 |
| `x.pad(w)` | integers and floats | `u8[.]` | at least `w` characters, spaces on the left |
| `x.zeroPad(w)` | integers and floats | `u8[.]` | at least `w` characters, zeros after any sign |
| `x.fixed(d)` | floats | `u8[.]` | `d` digits after the point |
| `c.utf8()` | `u32` | `u8[..~16]` | the code point as UTF-8 text |
| `x.sci(d)` | floats | | **not yet**: scientific notation |

```java
void main() {
    i64 n = 255;
    i64 neg = -42;
    i8 small = -1;
    println("${n.hex()} ${n.oct()} ${n.bin()} ${neg.hex()} ${small.hex()} ${small.bin()}");
    println("[${n.pad(6)}] [${neg.pad(6)}] [${neg.zeroPad(6)}] [${12345.pad(3)}]");
    println("${3.14159.fixed(2)} ${2.5.fixed(3)} [${2.5.pad(6)}] [${2.5.zeroPad(6)}]");
    u32 c = 0x1F600;
    println("${c.utf8()} ${'é'.utf8()} ${c.hex()}");
}
```

```output
ff 377 11111111 ffffffffffffffd6 ff 11111111
[   255] [   -42] [-00042] [12345]
3.14 2.500 [   2.5] [0002.5]
😀 é 1f600
```

- **A negative integer prints in two's complement**, as wide as its type: `-1` as an `i8`
  is `ff`, and `-42` as an `i64` is sixteen digits.
- **`pad` and `zeroPad` never cut**: a number wider than `w` prints whole.
- **`fixed` rounds as C's `printf` does**, to the nearest binary value: `2.675.fixed(2)` is
  `2.67`, since 2.675 is stored as 2.67499999..., and halves go to even: `0.5.fixed(0)` is
  `0`. Infinity and NaN print as `inf` and `nan` through `fixed`, though `${x}` prints them
  as `Infinity` and `NaN`.
- **Each method is offered only where it means something.** `n.fixed(2)` on an `i64` is an
  error that says `i64` has no method `fixed`, and so is `x.hex()` on a float.
- **Results whose width the type fixes are capped arrays**, which live inline and never
  touch a data stack: `u8[..~16] h = n.hex();`. `pad`, `zeroPad` and `fixed` depend on a
  value known only at run time, so they give a `u8[.]`, built in place:
  `u8[.] s = n.pad(8);`.
- **`utf8()` of a value that isn't a code point**, a surrogate or above `0x10FFFF`, writes
  `�` (U+FFFD). The design has it fail with `INVALID` too. Today it sets no status, and
  reading its status, as in `u8 st = @c.utf8();`, type-checks but has no code generated
  ([What isn't there yet](15-not-yet.md)).
- **Not yet:** `sci(d)` type-checks but has no code generated.

```java
void main() {
    u8[..~16] h = 48879.hex();
    u8[.] p = 42.pad(6);
    u8[.] f = 1.0.fixed(20);
    u32 big = 0x110000;
    println("${h} [${p}] ${f} ${big.utf8()}");
}
```

```output
beef [    42] 1.00000000000000000000 �
```

A program's own format is a function that returns text. It goes in `${...}` the same way:

```java
u8[.] money(i64 cents) {
    return "$${cents / 100}.${(cents % 100).zeroPad(2)}";
}

void main() {
    println("price: ${money(1999)}, tax: ${money(160)}");
}
```

```output
price: $19.99, tax: $1.60
```

## Comparing text

`==` and `!=` compare contents, byte by byte. `<`, `<=`, `>` and `>=` compare in byte
order, which for UTF-8 is code point order. Either side can be a `string` or a `u8` array.

```java
void main() {
    u8[..>] buf = [];
    buf.append("same");
    u8[..~8] small = "same";
    println("${"a" == "a"} ${buf == small} ${small == "same"} ${buf != "Same"}");
    println("${"abc" < "abd"} ${"b" >= "abc"} ${"" < "a"} ${"Z" < "a"} ${"ab" < "abc"}");
    println("${"abc".compareTo("abd")} ${"b".compareTo("a")} ${"x".compareTo("x")}");
    println("${"file2".compareNatural("file10")} ${"v1.9".compareNatural("v1.10")} ${"01".compareNatural("1")}");
}
```

```output
true true true true
true true true true true
-1 1 0
-1 -1 -1
```

- **`s.compareTo(other)`** gives an `i32` below 0, 0 or above 0, in the same byte order as
  `<`.
- **`s.compareNatural(other)`** compares runs of ASCII digits by their value and everything
  else byte by byte, as `ls -v` does: `"file2"` comes before `"file10"`. Equal numbers
  written differently, such as `01` and `1`, fall back to byte order.
- **Sorting strings** with `sort()` uses byte order; a comparator gives any other order:

```java
void main() {
    string[..>] files = ["file10", "file2", "File1", "file1"];
    files.sort();
    println("${files}");
    files.sort((string a, string b) => a.compareNatural(b) < 0);
    println("${files}");
}
```

```output
[File1, file1, file10, file2]
[File1, file1, file2, file10]
```

- **Not yet:** `equalsIgnoreCase(other)`, which compares ASCII letters without case,
  type-checks but has no code generated. Compare two `toLower()` copies instead.
- **`min(a, b)` and `max(a, b)` take strings**, in text order: `min("pear", "apple")` is
  `apple`.

## `switch` on strings

`switch` works on strings as it does on integers and enums: a case can list several
values, there is no fall-through, and as an expression it needs a `default`
([The basics](02-basics.md)).

```java
i64 score(string word) {
    return switch (word) {
        case "one" => 1;
        case "two", "deux" => 2;
        default => 0;
    };
}

void main() {
    println("${score("one")} ${score("deux")} ${score("three")}");
    string cmd = "stop";
    switch (cmd) {
        case "go", "start" => println("going");
        case "stop" => println("stopping");
        default => println("unknown: ${cmd}");
    }
    u8 b = 'e';
    string kind = switch (b) {
        case 'a', 'e', 'i', 'o', 'u' => "vowel";
        case ' ' => "space";
        default => "other";
    };
    println(kind);
}
```

```output
1 2 0
stopping
vowel
```

- **A byte switches on character literals**, which take the type `u8` there.
- **A `u8` array passed to a `string` parameter is switched on as text**, as `score(buf)`
  would be. Switching on a `u8` array directly type-checks but doesn't compile today; take a
  view first, `string v = buf; switch (v) { ... }` ([What isn't there yet](15-not-yet.md)).

## Text methods

Text methods take their receiver as a `string`, so they work on literals, views and every
`u8` array that converts to one. Results that are parts of the text are views into it, and
copy nothing; results that are new text are a `u8[.]`, built where the caller keeps it.

| Method | Result | Fails with | Today |
|---|---|---|---|
| `s.find(needle)` | `i64`, the first index | `MISSING` | yes |
| `s.find(needle, start)` | `i64`, the first index from `start` | `MISSING`; `INVALID` for a start outside `0..s.len` | yes |
| `s.rfind(needle)` | `i64`, the last index | `MISSING` | yes |
| `s.findAny(bytes)` | `i64`, the first byte that is one of `bytes` | `MISSING` | not yet |
| `s.contains(needle)` | `bool` | | yes |
| `s.startsWith(prefix)`, `s.endsWith(suffix)` | `bool` | | yes |
| `s.count(needle)` | `i64`, non-overlapping occurrences | | yes |
| `s.split(sep)` | `string[..>]` of views | | yes |
| `s.splitBy(sep)` | an iterator of views | | yes |
| `s.lines()` | an iterator of views | | yes |
| `s.words()` | an iterator of views | | yes |
| `s.splitFirst(sep)`, `s.splitLast(sep)` | `(string, string)` | | yes |
| `parts.join(sep)` | `u8[.]` | | not yet |
| `s.trim()`, `s.trimStart()`, `s.trimEnd()` | `string`, a view | | yes |
| `s.trimPrefix(p)`, `s.trimSuffix(p)` | `string`, a view | | yes |
| `s.compareTo(o)`, `s.compareNatural(o)` | `i32` | | yes |
| `s.equalsIgnoreCase(o)` | `bool` | | not yet |
| `s.toUpper()`, `s.toLower()` | `u8[.]` | | yes |
| `s.repeat(n)` | `u8[.]` | `INVALID` for a negative `n` | yes |
| `s.replaceAll(from, to)`, `s.replaceFirst(from, to)` | `u8[.]` | | not yet |
| `s.pad(w)`, `s.padEnd(w)` | `u8[.]` | | not yet |
| `s.codePoints()` | an iterator of `u32` | | yes |
| `s.isValidUtf8()` | `bool` | | yes |
| `s.codePointCount()` | `i64` | | not yet |
| `s.hex()` | `u8[.]`, the bytes in hex | | yes |

"Not yet" methods type-check, and `zink run` then says `the C backend doesn't support ...
yet`. They are listed in [What isn't there yet](15-not-yet.md). The full signatures are in
[STDLIB §3](../STDLIB.md#3-text-v1-in-zinklang).

### Searching

```java
void main() {
    string s = "a,b,,c";
    i64 i @e1 = s.find(",");
    i64 j @e2 = s.find(",", 2);
    i64 k @e3 = s.rfind(",");
    println("${i} ${j} ${k} ${e1} ${e2} ${e3}");
    i64 x @missing = s.find("x");
    i64 y @bad = s.find(",", 99);
    println("${x} ${status.name(missing)} ${y} ${status.name(bad)}");
    println("${s.contains(",,")} ${s.startsWith("a,")} ${s.endsWith(",c")}");
    println("${s.count(",")} ${"aaaa".count("aa")} ${"banana".count("ana")}");
}
```

```output
1 3 4 0 0 0
0 MISSING 0 INVALID
true true true
3 2 1
```

- **Needles are strings.** A character literal is an integer, so `s.find(',')` is an error,
  `expected string, found a character literal`; write `s.find(",")`.
- **A failed `find` gives 0 with its status**, as every failure gives the zero value
  ([Errors and statuses](07-errors.md)). Read the status before using the index.
- **`find(needle, start)` takes a start from 0 to `s.len`.** `"abc".find("c", 3)` fails with
  `MISSING`; a start below 0 or past the end fails with `INVALID`.
- **The empty needle** is found at 0, and `count("")` gives `s.len + 1`.
- **`contains` also takes a byte**, as the array method does: `s.contains(',')` is the same
  test as `s.contains(",")`.

### Splitting

Every part is a view into the text, so splitting copies nothing. `split` gives an array of
them; `splitBy`, `lines` and `words` are lazy iterators, which a for-each walks without
building an array ([Maps, sets, pools and iterators](09-collections.md)).

```java
void main() {
    string s = "a,b,,c";
    string[..>] parts = s.split(",");
    println("${parts.len} ${parts} ${parts[3]}");
    for (string p : s.splitBy(",")) {
        print("[${p}]");
    }
    println("");
    for (string l : "one\ntwo\r\n\nthree\n".lines()) {
        print("<${l}>");
    }
    println("");
    for (string w : "  a  b\tc\n".words()) {
        print("<${w}>");
    }
    println("");
    (string k, string v) = "k=v=w".splitFirst("=");
    (string base, string ext) = "file.tar.gz".splitLast(".");
    (string name, string none) = "README".splitLast(".");
    println("${k}|${v} ${base}|${ext} ${name}|${none}|");
}
```

```output
4 [a, b, , c] c
[a][b][][c]
<one><two><><three>
<a><b><c>
k|v=w file.tar|gz README||
```

- **`split` and `splitBy` keep empty parts**: `"a,,b"` gives `a`, an empty part and `b`; a
  separator at the end gives an empty last part; and `""` gives one empty part. An empty
  separator gives the whole text as one part.
- **`lines()` ends a line at `\n` or `\r\n`**, and leaves the newline out. A final newline
  makes no empty last line, a last line without one still counts, and an empty text has no
  lines. Empty lines in the middle are kept.
- **`words()` splits at runs of ASCII whitespace** (space, `\t`, `\n`, `\v`, `\f`, `\r`) and
  never gives an empty word.
- **`splitFirst` and `splitLast` never fail.** With no separator they give the whole text
  and `""`, so `"home".splitFirst(" ")` is `("home", "")`. Use `contains` first when the
  difference between `"home"` and `"home "` matters.
- **Not yet:** `parts.join(sep)`. Append the parts to a buffer instead:

```java
void main() {
    string[..>] parts = ["x", "y", "z"];
    u8[..>] joined = [];
    for (i64 i = 0; i < parts.len; i++) {
        if (i > 0) {
            joined.append(", ");
        }
        joined.append(parts[i]);
    }
    println(joined);
}
```

```output
x, y, z
```

A view from `split` is rooted at the text it came from, so the text must outlive the array
of parts, and can't be removed from while the parts are used. Text that is split usually
lives in one `u8[..>]` buffer the whole time, as in [`wordfreq`](../examples/wordfreq.zn).

### Trimming

```java
void main() {
    string s = "  hi  ";
    println("[${s.trim()}] [${s.trimStart()}] [${s.trimEnd()}] [${"\t\r\n x y \n".trim()}]");
    println("${"prefix-body".trimPrefix("prefix-")} ${"file.txt".trimSuffix(".txt")} ${"abc".trimPrefix("x")}");
    u8[..>] line = [];
    line.append("  padded  ");
    string t = line.trim();
    println("[${t}] ${t.len}");
}
```

```output
[hi] [hi  ] [  hi] [x y]
body file abc
[padded] 6
```

- **Trimming removes ASCII whitespace**: space, `\t`, `\n`, `\v`, `\f` and `\r`.
- **`trimPrefix` and `trimSuffix` give the text unchanged** when it doesn't start or end
  with what they're given.
- **The result is a view into the text**, rooted at it: `t` above is a view of `line`.

### Transforming

`toUpper`, `toLower` and `repeat` make new text, a `u8[.]`. In `${...}` it goes straight to
the output; declared, it is built in the variable.

```java
void main() {
    println("${"MiXed".toUpper()} ${"MiXed".toLower()} ${"héllo wörld".toUpper()} [${"ab".repeat(3)}] [${"ab".repeat(0)}]");
    u8[.] key = "Apple".toLower();
    println(key);
    u8[.] r @err = "ab".repeat(-1);
    println("[${r}] ${status.name(err)}");
}
```

```output
MIXED mixed HéLLO WöRLD [ababab] []
apple
[] INVALID
```

- **Case mapping is ASCII only.** Other bytes, such as the two of `é`, stay as they are.
  Unicode case mapping waits for the Unicode tables (STDLIB §3.5).
- **`repeat` of a negative count fails with `INVALID`**, and gives no text.
- **Not yet:** `replaceAll`, `replaceFirst`, `pad(w)` and `padEnd(w)` on text. Number
  `pad` works (above).

### Bytes

Methods on a `u8` test and map ASCII bytes. Every other byte is a non-letter, and maps to
itself.

| Method | True for, or gives | Today |
|---|---|---|
| `b.isDigit()` | `0` to `9` | yes |
| `b.isAlpha()` | `A` to `Z`, `a` to `z` | yes |
| `b.isUpper()`, `b.isLower()` | `A` to `Z`; `a` to `z` | yes |
| `b.isSpace()` | space, `\t`, `\n`, `\v`, `\f`, `\r` | yes |
| `b.toUpper()`, `b.toLower()` | the letter in the other case; any other byte unchanged | yes |
| `b.isHexDigit()`, `b.isAlnum()`, `b.isPrint()` | | not yet |

```java
void main() {
    u8[..>] text = [];
    text.append("Hello, World 42");
    i64 letters = 0;
    i64 digits = 0;
    for (u8& b : text) {
        if (b.isAlpha()) {
            letters++;
        }
        if (b.isDigit()) {
            digits++;
        }
        b = b.toUpper();
    }
    println("${text} ${letters} ${digits} ${text[0].isUpper()} ${((u8) ' ').isSpace()}");
}
```

```output
HELLO, WORLD 42 10 2 true true
```

The loop changes the buffer in place, through a reference to each byte
([Arrays](04-arrays.md)).

## UTF-8 and code points

Strings are UTF-8 bytes, and nothing checks that they are valid UTF-8 until a program asks.
A code point is a `u32`. `codePoints()` decodes text into code points, lazily, and
`utf8()` encodes one back into text.

```java
void main() {
    string s = "héllo €";
    i64 n = 0;
    for (u32 c : s.codePoints()) {
        print(" ${c.hex()}");
        n++;
    }
    println("");
    println("${s.len} bytes, ${n} code points, valid ${s.isValidUtf8()}");
    u32[..>] cps = [];
    for (u32 c : "naïve".codePoints()) {
        cps +> c;
    }
    cps.reverse();
    u8[..>] out = [];
    for (u32 c : cps) {
        out.append("${c.utf8()}");
    }
    println(out);
}
```

```output
 68 e9 6c 6c 6f 20 20ac
10 bytes, 7 code points, valid true
evïan
```

- **A byte that can't start a sequence, or a sequence cut short, decodes to U+FFFD** (`fffd`
  in hex) for its first byte, and decoding goes on with the next byte, so a loop over bad
  text still ends.
- **`isValidUtf8()` is strict**: it rejects overlong forms, encoded surrogates and values
  above `10FFFF`, as well as bad and cut-off sequences.
- **`codePoints()` is not as strict.** It decodes an overlong form, such as the bytes `C0 AF`,
  to its value (`2f`), and an encoded surrogate or a value above `10FFFF` to that value, where
  STDLIB §3.5 has U+FFFD. Check `isValidUtf8()` first where that matters.

```java
void main() {
    u8[..>] b = [];
    b +> 0xFF;
    b +> 'a';
    b +> 0xC3;
    print("${b.isValidUtf8()}:");
    for (u32 c : b.codePoints()) {
        print(" ${c.hex()}");
    }
    println("");
    b.clear();
    b +> 0xC0;
    b +> 0xAF;
    print("overlong ${b.isValidUtf8()}:");
    for (u32 c : b.codePoints()) {
        print(" ${c.hex()}");
    }
    println("");
}
```

```output
false: fffd 61 fffd
overlong false: 2f
```

- **`hex()` on text gives its bytes in hex**, two digits each: `"hi".hex()` is `6869`.
- **Not yet:** `codePointCount()`, `utf8()` on an array of code points, and
  `appendUtf8`. Count with a loop over `codePoints()`, and encode one code point at a time
  with `utf8()`, as above.
- **Unicode properties** (`isAlphabetic`, `isWhitespace`, case mapping on `u32`, display
  width) come later, with the Unicode tables (STDLIB §3.5).
