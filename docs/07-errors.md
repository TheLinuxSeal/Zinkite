# 7. Errors and statuses

Zinkite has no exceptions. Every call returns a `u8` **status** beside its value: 0 when it
succeeded, and any other number when it failed, saying why. This chapter covers writing a
status with `#`, reading one with `@`, what a failure leaves in the value, repeat loops that
run until a call fails, what happens to a failure nothing reads, catching one deep inside a
call with `@@`, the trail `@@@` gives, and the named codes. The reasons, and the designs that
were rejected, are in [DESIGN.md §7](../DESIGN.md#7-the-status-code-decided) and
[Appendix A.2](../DESIGN.md#a2-exceptions).

## The status of a call

A function fails by returning a status with its value. The caller reads it with `@`:

```java
i64 half(i64 n) {
    if (n % 2 != 0) {
        return #status.INVALID 0;
    }
    return n / 2;
}

void main() {
    i64 a @err = half(10);
    println("half(10) is ${a}, status ${err}");
    i64 b @why = half(7);
    println("half(7) is ${b}, status ${why}: ${status.message(why)}");
}
```

```output
half(10) is 5, status 0
half(7) is 0, status 22: Invalid argument
```

- **The default is 0.** A function that never writes a status returns 0, and so does one
  that falls off its end.
- **The status belongs to the call, not the function.** It is read where the call is made,
  and if it is nonzero and nothing reads it, the program stops there (see
  [A failure nothing reads stops the program](#a-failure-nothing-reads-stops-the-program)).
- **Nothing propagates by itself.** Passing a status on to your own caller is written out,
  with `#` or `##`.
- **By convention, nonzero means something went wrong**: 1 for a failure with no
  particular reason, and a [named code](#named-codes) when there is one, such as
  `status.INVALID` (22) above.
- **It costs a register.** The status is a hidden second return value, and a function that
  never sets one carries none.

## Writing a status: `#`

`#` after `return` gives the status, and the value follows it:

```java
i64[..~] stack = [];

void push(i64 x) {
    if (x < 0) {
        return #1;                          // a void function: the status alone
    }
    stack +> x;
}

i64 find(string s, u8 c) {
    for (i64 i = 0; i < s.len; i++) {
        if (s[i] == c) {
            return i;                       // status 0
        }
    }
    return #status.NOT_FOUND -1;            // a named code, and a value
}

i64 limit(i64 n) {
    bool tooBig = n > 100;
    return #(tooBig) n;                     // a bool: 1 when true, 0 when false
}

i64 limit2(i64 n) {
    u8 why = n > 100 ? status.RANGE : 0;
    return #why n;                          // a u8 variable
}

i64 limit3(i64 n) {
    return #(n > 100 ? status.RANGE : 0) n; // a u8 expression in parentheses
}

void main() {
    u8 e1 = @push(-1);
    u8 e2 = @push(5);
    i64 f @e3 = find("hello", 'z');
    println("${e1} ${e2} ${f} ${e3} ${stack}");
    i64 a @e4 = limit(200);
    i64 b @e5 = limit2(200);
    i64 c @e6 = limit3(50);
    println("${a} ${e4} ${b} ${e5} ${c} ${e6}");
}
```

```output
1 0 -1 2 [5]
200 1 200 34 50 0
```

- **`#` takes** an integer literal from 0 to 255, a named code (`#status.FULL`), a `u8` or
  `bool` variable (`#why`), or a `u8` or `bool` expression in parentheses (`#(n > 100)`).
- **A `bool` counts as 0 or 1.**
- **`return #0 x;`** is an explicit success, the same as `return x;`.
- **In a function or constructor that returns nothing**, `return #code;` is an early exit
  with that status.
- **The value is still returned** with a failing status: `limit(200)` gives 200 with
  status 1. What the value should be on failure is up to the function (see
  [The value on failure](#the-value-on-failure)).
- **`main` can't set a status.** Its exit status is the value it returns (see
  [The exit status](#the-exit-status)).

Other integer types aren't statuses. The compiler says how to convert:

```java
i64 f(i64 n) {
    i64 code = 3;
    return #code n;
}

void main() {
    i64 x @_ = f(1);
}
```

```error
example.zn:3:13: error: `#` takes a u8 or bool variable, and `code` is i64
help: write `#((u8) n)`, or `#(n != 0)` (DESIGN §7.2)
```

A literal must fit in a `u8` (`return #300 n;` is "a status is a u8, from 0 to 255"), and a
name after `status.` must be one of the [named codes](#named-codes) (`status.NOPE` is
"`status.NOPE` isn't a status code").

## Passing a status on: `return ##`

`return ## e;` returns `e`'s value and the status `@` would read from it. It means exactly
`T v @err = e; return #err v;`:

```java
i64 parsePort(string s) {
    i64 n @err = i64.parse(s);
    if (err) {
        return #status.INVALID 0;
    }
    if (n < 1 || n > 65535) {
        return #status.RANGE 0;
    }
    return n;
}

i64 portOf(string addr) {
    i64 colon @none = addr.find(":");
    if (none) {
        return #status.MISSING 0;
    }
    return ## parsePort(addr[colon + 1:]);
}

i64 divide(i64 a, i64 b) {
    return ## (a / b);
}

i64 at(i64[..>]& xs, i64 i) {
    return ## xs[i];
}

i64[..~] stack = [];

void push(i64 x) {
    if (x < 0) {
        return #status.INVALID;
    }
    stack +> x;
}

void pushBoth(i64 x, i64 y) {
    push(x);
    return ## push(y);
}

void main() {
    string[.] addrs = ["example.com:8080", "example.com", "example.com:http", "example.com:99999"];
    for (string a : addrs) {
        i64 p @err = portOf(a);
        println("${a}: ${p} ${status.name(err)}");
    }
    i64[..>] xs = [1, 2, 3];
    i64 q @e1 = divide(7, 0);
    i64 r @e2 = at(xs, 5);
    u8 e3 = @pushBoth(1, -2);
    println("${q} ${status.name(e1)}, ${r} ${status.name(e2)}, ${status.name(e3)} ${stack}");
}
```

```output
example.com:8080: 8080 OK
example.com: 0 MISSING
example.com:http: 0 INVALID
example.com:99999: 0 RANGE
0 DOMAIN, 0 RANGE, INVALID [1]
```

- **It passes on one layer**, as `@` reads it: the call's own failure, and any failure in
  the expression, such as an index (`return ## xs[i];`) or a division
  (`return ## (a / b);`, with the operator expression in parentheses).
- **In a function that returns nothing**, `e` is a call that returns nothing, and only its
  status is passed on.
- **It continues the failure's trail**, so a stop message says "`portOf` passed it on" (see
  [The trail](#the-trail)).
- **`return f();` is not the same.** It is a claim that `f` doesn't fail here, and if it
  does, the program stops at that line. A status is passed on only where `#` or `##` is
  written, so whether a function can fail never depends on a line that looks like any other
  `return` ([DESIGN §7.2](../DESIGN.md#72-writing-a-status-)).
- **`e` must be able to fail.** `return ## n;` is "`##` passes on the status of something
  that can fail, and this can't", and a call to a function that never sets a status is an
  error too:

```java
i64 twice(i64 n) {
    return n * 2;
}

i64 f(i64 n) {
    return ## twice(n);
}

void main() {
    i64 x @_ = f(1);
}
```

```error
example.zn:6:15: error: this never sets a status, so `##` has nothing to pass on
help: write `return f();` (DESIGN §7.2)
```

`@` after `return` is an error that points at `##`: `return @g(n);` is "`@` after `return`
reads a status as a value", with the help "to pass a call's status on with its value, write
`return ## f();`".

## Reading a status: `@`

`@name` beside a destination takes the status into `name`, and `@` in front of an
expression gives its status:

```java
void main() {
    i64[..~4] st = [];
    i64 v @e1 = st.pop();                       // declares v and e1
    println("pop on empty: ${v} ${status.name(e1)}");
    @e1 = st +> 1 +> 2 +> 3 +> 4 +> 5;          // the status only, into e1
    println("push past the cap: ${status.name(e1)} ${st}");
    i64 top = 0;
    top @e1 = st.pop();                         // assigns both
    println("${top} ${e1}");
    u8 e2 = @st.pop();                          // @ in an expression; the value is dropped
    println("${e2} ${st}");
    if (@st.pop()) {
        println("not printed: there was one to pop");
    }
    println("${st}");
}
```

```output
pop on empty: 0 EMPTY
push past the cap: FULL [1, 2, 3, 4]
4 0
0 [1, 2]
[1]
```

- **In a declaration**, `T x @err = f();` declares `err` along with `x`. A status is always
  a `u8` and needs no type written.
- **In an assignment**, `x @err = f();` and `@err = f();` assign a status that already
  exists. Assigning one that doesn't is an error that shows both forms of declaring it.
- **In an expression**, `@f()` gives the status directly: `u8 e = @f();` declares a status
  on its own, and `if (@f())` tests it. The call still runs, and its value is dropped.
- **Each status variable is an ordinary local** once declared: `i64 x @e = f(); u8 e = 0;`
  is "`e` is already declared in this block".
- **A chain keeps the first failure.** The status of `st +> 1 +> 2 +> 3 +> 4 +> 5` is the
  first nonzero one, and later steps don't overwrite it.
- **`@` is a high-precedence prefix**: `@st.pop()` means `@(st.pop())`. An operator
  expression needs parentheses, `@(a +> x)` or `@(n / d)`. Without them, `@` applies to the
  first operand, and `@a +> 1` on an array `a` is "`@` reads the status of something that
  can fail, and this can't", with the help "for an operator expression, put it in
  parentheses: `@(a / b)`".

### `@` reads every failure in its expression

Not only calls fail. An index or a slice out of range, an overflow in `+#`, `-#` or `*#`, a
cast that doesn't fit, and a division or remainder by zero each fail with a status, and `@`
reads them all:

```java
i64 noisy(i64 n) {
    println("noisy(${n}) runs");
    return n;
}

void main() {
    i64[..>] xs = [10, 20, 30];
    i64 x @e1 = xs[5];
    println("index: ${x} ${status.name(e1)}");
    string s @e2 = "hello"[2:9];
    println("slice: [${s}] ${status.name(e2)}");
    i64 big = i64.MAX;
    i64 y @e3 = big +# 1;
    println("overflow: ${y} ${status.name(e3)}");
    i64 n = 300;
    u8 b @e4 = (u8) n;
    println("cast: ${b} ${status.name(e4)}");
    i64 zero = 0;
    i64 q @e5 = 10 / zero;
    u8 e6 = @(10 % zero);
    println("division: ${q} ${status.name(e5)} ${status.name(e6)}");
    i64 z @e7 = xs[9] + noisy(1);
    println("skipped: ${z} ${status.name(e7)}");
    i64 w @e8 = noisy(2) + xs[1];
    println("${w} ${e8}");
}
```

```output
index: 0 RANGE
slice: [] RANGE
overflow: 0 RANGE
cast: 0 RANGE
division: 0 DOMAIN DOMAIN
skipped: 0 RANGE
noisy(2) runs
22 0
```

- **The first failure anywhere in the expression is caught, and the rest is skipped.**
  `noisy(1)` never ran, since `xs[9]` failed first.
- **In `@f(g())`**, the status is `g`'s if `g` fails, and then `f` isn't called; otherwise
  it is `f`'s.
- **These checks fail with**:

  | Failure | Status |
  |---|---|
  | a call that fails | its own |
  | an index or a slice out of range | `RANGE` |
  | an overflow in `+#`, `-#` or `*#` | `RANGE` |
  | a cast that doesn't fit, such as `(u8) n` when `n` is 300 | `RANGE` |
  | `x / 0` and `x % 0` | `DOMAIN` |

- **The forms that don't fail** remain for code that wants another outcome: `+`, `-` and
  `*`, which wrap; `+|`, `-|` and `*|`, which saturate; the casts `(u8%) n`, which wraps,
  and `(u8|) n`, which saturates; `u8.from(n)`, which fails with `RANGE` as a call does;
  `a[%i]`, `a[|i]` and `a.getOr(i, fallback)` for indexes; and `x.divOr(d, fallback)`.
  [The basics](02-basics.md) and [Arrays](04-arrays.md) cover them.

### Discarding with `_`

`_` is a destination that discards. It can be given anything, any number of times, and can
never be read:

```java
i64 parse(string s) {
    return ## i64.parse(s);
}

void main() {
    i64 port @_ = parse("http");        // keep the value, ignore the status: 0 here
    i64[..~] xs = [];
    xs +> 1;
    @_ = xs.pop();                      // a call made for its effect
    _ @_ = parse("x");                  // ignore both
    _ = parse("12");                    // ignore the value; a failure would still stop
    i64 n = 0;
    xs +> 1 +> 2;
    while (var _ : xs ->) {
        n++;
    }
    println("${port} ${xs.len} ${n}");
}
```

```output
0 0 2
```

- **`@_` is a decision to ignore the failure.** The value is then whatever the call
  returned on failure, usually the zero value.
- **`_ = f();` discards only the value.** The status is still unread, so a failure stops
  the program.
- **`_` also works** as a parameter name, a destructuring target, a for-each variable and a
  repeat-loop variable written `var _`.
- **Reading it is an error**: `i64 y = _;` is "`_` can't be read; it only discards". So is
  a compound assignment such as `_ += 1`.
- **Only a lone `_` is special.** `_count` is an ordinary name.

## Statuses as conditions

Unsigned integers work as conditions, so `if (err)` tests whether a call failed:

```java
void main() {
    i64[..~] left = [];
    i64[..~] right = [];
    right +> 7;
    u8 err = @left.pop();
    if (err) {
        println("left failed: ${status.name(err)}");
    }
    if (!@right.pop()) {
        println("right had one");
    }
    right +> 8;
    if (@left.pop() || @right.pop()) {
        println("either failed; right still holds ${right.len}");
    }
    if (@left.pop() && @right.pop()) {
        println("not printed");
    } else {
        println("not both failed; right holds ${right.len}");
    }
}
```

```output
left failed: EMPTY
right had one
either failed; right still holds 1
not both failed; right holds 0
```

- **In `if`, `while`, `do`, the condition of `for` and `?:`**, and as operands of `!`, `&&`
  and `||`, a `u8`, `u16`, `u32` or `u64` is false when it is 0 and true otherwise.
- **`@a || @b` is true when either failed, and `@a && @b` when both did.** Read them as "if a
  failed", not "if a succeeded". Both stop as early as `||` and `&&` always do: above, the first `||` never
  popped `right`, since `left` had already failed.
- **Signed integers and floats aren't conditions**: `if (n)` on an `i64` is "a signed
  integer (i64) isn't a condition", with the help "write `n != 0`".
- **There's no conversion outside conditions**: `bool b = err;` is "expected bool, found
  u8". Write `err != 0`.
- **An assignment can't be a condition**: `if (x = 0)` is "an assignment can't be a
  condition", since with integers as conditions it is usually a typo for `==`.
- **A bare call in a condition tests its value**; `@` in front tests its status.

## The value on failure

The value is always defined. Every `return` that has a value gives one, whatever its
status, so `x @err = f();` never leaves `x` undefined:

```java
import zink.io.*;

class Point {
    i64 x;
    i64 y;
}

i64 clampTo(i64 n, i64 hi) {
    if (n > hi) {
        return #status.RANGE hi;
    }
    return n;
}

void main() {
    i64 a @e1 = clampTo(500, 255);
    println("${a} ${status.name(e1)}");
    Point[..>] pts = [new Point(1, 2)];
    Point p @e2 = pts[3];
    println("${p.x} ${p.y} ${status.name(e2)}");
    Point& r @e3 = pts[3];
    r.x = 99;
    println("${pts[0].x} ${r.x} ${status.name(e3)}");
    f64 v @e4 = f64.parse("1.5x");
    println("${v} ${status.name(e4)}");
    File f @e5 = File.open("/nonexistent/file");
    u8 e6 = @f.close();
    println("${status.name(e5)} ${status.name(e6)}");
}
```

```output
255 RANGE
0 0 RANGE
1 99 RANGE
0.0 INVALID
NOT_FOUND BAD_HANDLE
```

- **A function of the program's own returns what its `return` gave**, such as `hi` from
  `clampTo`.
- **The standard library returns the zero value on failure**: `0`, `false`, an empty view,
  or the zero value of a class. A zero-value `File` is a closed one, so `close()` on it
  fails with `BAD_HANDLE`.
- **When `@` catches a failure partway through its expression**, the rest isn't evaluated
  and the value is the zero value.
- **A reference then refers to a zero value of its own**, not to any element, so writing
  through it changes nothing else: `r.x = 99` above left `pts[0]` alone.

## Repeat loops

`while (T x : call)` makes the call before every pass. While its status is 0, the value is
bound to `x` and the body runs; at the first nonzero status the loop ends without running
the body. A status capture after the header says why it ended:

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

i64[..>] readings = [3, 8, -1, 4];

i64 reading(i64 i) {
    if (i >= readings.len) {
        return #status.END 0;
    }
    if (readings[i] < 0) {
        return #status.INVALID 0;
    }
    return readings[i];
}

void main() {
    Countdown c = new Countdown(3);
    while (i64 x : c.next()) {
        print("${x} ");
    }
    println("");
    i64[..~] stack = [];
    stack +> 1 +> 2 +> 3;
    while (i64 x : stack ->) {
        print("${x} ");
    }
    println("");
    i64 i = 0;
    i64 sum = 0;
    while (i64 r : reading(i)) @why {
        sum += r;
        i++;
    }
    if (why != status.END) {
        println("stopped at ${i} by ${status.name(why)}, with ${sum} so far");
    }
    i64 j = 0;
    while (i64 r : reading(j)) @why2 {
        if (r > 5) {
            break;
        }
        j++;
    }
    println("break gives ${why2}, at ${j}");
}
```

```output
3 2 1 
3 2 1 
stopped at 2 by INVALID, with 11 so far
break gives 0, at 1
```

- **The source is anything `@` can read**: a call, an array operator such as `stack ->`, or
  an operator expression in parentheses. The loop ends exactly when `@` would give a
  nonzero status.
- **The call is made again on every pass**, with its arguments evaluated each time.
  `continue` makes the next call; `break` leaves without one.
- **The capture** (`@why`) is declared in the enclosing block, since it is read after the
  loop. It holds the nonzero status of the last call, or 0 when a `break` ended the loop.
  With a capture, the body must be a block.
- **`@@why` after the header** also ends the loop at a failure nothing read inside the call
  (see [`@@`](#catching-what-nothing-read-)).
- **The call must be able to fail**, or the loop could never end:

```java
i64 g(i64 n) {
    return n;
}

void main() {
    while (i64 x : g(1)) {
        println("again");
    }
}
```

```error
example.zn:6:20: error: this never sets a status, so the loop would never end
help: use `while (true)` with a `break` (DESIGN §9.4)
```

A for-each over an iterator is a repeat loop on its `next()`, so an iterator's `next()`
ends the loop the same way, with `END` by convention. [Maps, sets, pools and
iterators](09-collections.md) covers iterators.

## A failure nothing reads stops the program

A failure that nothing reads doesn't carry on with a zero value: the program stops at the
line where it happened. The message names what failed and its status, then the failure's
trail through the program's own functions:

```java
i64 parsePort(string s) {
    i64 n @err = i64.parse(s);
    if (err) {
        return #status.INVALID 0;
    }
    return n;
}

i64 portOf(string addr) {
    i64 colon @none = addr.find(":");
    if (none) {
        return #status.MISSING 0;
    }
    return ## parsePort(addr[colon + 1:]);
}

void main() {
    println("before");
    i64 p = portOf("example.com:http");
    println("after ${p}");
}
```

```output exit=1
before
```

```stderr exit=1
example.zn:19: `portOf` failed with INVALID, and nothing read its status
    `parsePort` returned it at example.zn:4
    `portOf` passed it on at example.zn:14
```

A check in an expression stops the program with a message of its own:

```java
void main() {
    i64[..>] xs = [1, 2, 3];
    println("start");
    i64 i = 5;
    i64 x = xs[i];
    println("${x}");
}
```

```stderr exit=1
example.zn:5: index 5 is out of range for length 3
```

- **The messages for the checks** are:

  | Failure | Message |
  |---|---|
  | an index out of range | `index 5 is out of range for length 3` |
  | a slice out of range | `slice 2:9 is out of range for length 3` |
  | an overflow in `+#`, `-#` or `*#` | `integer overflow` |
  | a cast that doesn't fit | `the value doesn't fit in u8` |
  | `x / 0` or `x % 0` | `division by zero` |
  | a library call | `` `pop` failed with EMPTY, and nothing read its status `` |

- **The program flushes its output and closes its open files**, so what it printed before
  the stop isn't lost, and **exits with status 1**.
- **It stops at the caller's line**, not inside the library or the function that failed.
  Whether a failure is expected or a bug is the caller's decision: reading the status says
  it's expected, and not reading it says it can't happen here.
- **So there is no warning for an unread status.** `x = a.pop();` is a claim that the stack
  isn't empty, and a call whose arguments can't make it fail, such as `rng.range(1, 7)`,
  needs nothing written.
- **The cost** is one predictable branch after a call that can fail and whose status
  nothing reads.

Returning from `main` ends a program, and a failure nothing reads stops it. Nothing else
stops a program, and there is no `exit`: a function deep inside fails with a status, and its
callers pass it up. A static initializer that fails stops the program before `main` runs.
What lies outside the language ends a program too, and no program can catch it: a stack
overflow, which ends it with `SIGSEGV`; a signal, such as `kill` or Ctrl-C; and a C function
that crashes or exits ([Calling C](12-calling-c.md)).

Writing to stdout after the reading end of a pipe has closed, as in `zink run tool.zn |
head`, ends the program quietly, the way standard Unix tools do, and the shell sees the
usual SIGPIPE status, 141.

### The trail

The trail says where a failure went through the program's own functions:

```java
i64 parsePort(string s) {
    i64 n @err = i64.parse(s);
    if (err) {
        return #status.INVALID 0;
    }
    return n;
}

i64 portOf(string addr) {
    i64 colon @none = addr.find(":");
    if (none) {
        return #status.MISSING 0;
    }
    i64 p = parsePort(addr[colon + 1:]);
    return p;
}

i64 config(string addr) {
    i64 p @@err = portOf(addr);
    if (err) {
        return #224 0;
    }
    return p;
}

void main() {
    i64 p = config("example.com:http");
    println("${p}");
}
```

```stderr exit=1
example.zn:27: `config` failed with 224, and nothing read its status
    `parsePort` returned INVALID at example.zn:4
    `portOf` didn't read INVALID at example.zn:14
    `config` returned it at example.zn:21
```

- **The first entry is where the failure started.** Each line after it is a function the
  failure went through, with its line: "returned" where a failing `return` gave it, "passed
  it on" where the same code went up again, and "didn't read" where nothing read it, which
  abandoned the call (see [`@@`](#catching-what-nothing-read-)). An entry whose code differs
  from the one in the first line names its code, as `INVALID` here.
- **A failing `return` continues the trail** of the failure the function read last, when
  its code is a status the function read (`return #err v;`, `return ## f();`) or when it is
  in a branch that tests one, as `if (err) { return #224 0; }` above. Any other failing
  `return` starts a new trail.
- **The library's functions add no entries**, so a failure from the library, or from a
  check in the expression itself, starts its trail at the line that reads it.
- **A trail longer than 32 entries** keeps its first 24 and its last 8, and the message
  says how many it left out, as `... 10 more`.
- **A method is named with its class**, as `Account.withdraw`, and a lambda as `lambda`.

## Catching what nothing read: `@@`

`@` catches one layer: the call's own status, and the checks in the expression. A failure
that nothing read *inside* the call still stops the program. `@@` catches that too, at any
depth, and abandons the calls in between:

```java
i64 lookup(i64[..>]& table, i64 i) {
    defer println("  lookup ends");
    i64 v = table[i];
    println("  lookup found ${v}");
    return v;
}

i64 total(i64[..>]& table, i64[.] wanted) {
    defer println("  total ends");
    i64 sum = 0;
    for (i64 i : wanted) {
        sum += lookup(table, i);
    }
    return sum;
}

void main() {
    i64[..>] table = [10, 20, 30];
    i64 a @@e1 = total(table, [0, 2]);
    println("${a} ${status.name(e1)}");
    i64 b @@e2 = total(table, [1, 5, 0]);
    println("${b} ${status.name(e2)}");
}
```

```output
  lookup found 10
  lookup ends
  lookup found 30
  lookup ends
  total ends
40 OK
  lookup found 20
  lookup ends
  lookup ends
  total ends
0 RANGE
```

- **The innermost `@@` around the calls that led to the failure catches it.** The calls in
  between are abandoned at the line where it happened: `table[5]` in `lookup`.
- **Nothing after that line runs in them, except their `defer`s**, innermost first, which
  see their variables as they were. The second `lookup` printed "ends" but not "found".
- **The value is the zero value**, as when `@` catches.
- **What they were changing may be left half-changed**: an array they were filling through
  a `&` parameter, or an object a `mut` method was updating. So `@@` suits a boundary whose
  work can be thrown away, such as one request of a server, one test, or one command of a
  REPL, more than everyday error handling. `examples/lisp.zn` evaluates each form of its
  REPL with `value @@failed = lisp.eval(form, NIL);`, so an unbound name or a division by
  zero deep in the interpreter fails that form and the session goes on.
- **The cost** is only at `@@` itself, which saves a jump point with `setjmp`, a few
  nanoseconds, and on the failing path.

### `@`, `@@` and `@@@` together

The forms can be combined, each at most once, in any order:

| Form | Catches | Gives |
|---|---|---|
| `x @a = f();` | `f`'s own failure, and every failure in the expression | `a`: the status |
| `x @@b = f();` | also a failure nothing read inside `f` | `b`: the outermost failure |
| `x @@@t = f();` | the same as `@@` | `t`: the trail, a `Failure[..~32]` |
| `x @a @@b @@@t = f();` and the other pairs | the same as `@@` | each |

```java
i64 parsePort(string s) {
    i64 n @err = i64.parse(s);
    if (err) {
        return #status.INVALID 0;
    }
    return n;
}

i64 portOf(string addr) {
    i64 colon @none = addr.find(":");
    if (none) {
        return #status.MISSING 0;
    }
    i64 p = parsePort(addr[colon + 1:]);
    return p;
}

void main() {
    string[.] addrs = ["example.com:8080", "example.com", "example.com:http"];
    for (string a : addrs) {
        i64 p @own @@any @@@trail = portOf(a);
        println("${a}: ${p}, own ${status.name(own)}, any ${status.name(any)}, trail ${trail.len}");
        for (Failure f : trail) {
            string how = f.unread ? ", unread" : "";
            println("    ${f.function} ${status.name(f.code)} at ${f.file}:${f.line}${how}");
        }
    }
    if (@@portOf("x:y")) {
        println("@@ in an expression: ${status.name(@@portOf("x:y"))}");
    }
}
```

```output
example.com:8080: 8080, own OK, any OK, trail 0
example.com: 0, own MISSING, any MISSING, trail 1
    portOf MISSING at example.zn:12
example.com:http: 0, own ABANDONED, any INVALID, trail 2
    parsePort INVALID at example.zn:4
    portOf INVALID at example.zn:14, unread
@@ in an expression: INVALID
```

- **`@` alone is the only form that catches one layer.** Any capture with `@@` or `@@@`
  catches everything, and nothing stops the program.
- **`a` beside `@@` or `@@@`** is `f`'s own status when `f` finished, and `ABANDONED` (200)
  when a failure nothing read inside it stopped it; `b` is then that failure. So `a` is
  never 0 when something failed, and `a != b` says that `f` was stopped rather than failing
  on its own.
- **In an expression**, `@@f()` gives what `@@b` would. `@@@` gives an array, so it is only
  a destination: `u8 t = @@@g(1);` is "`@@@` can't read a status here".
- **`@@_`** catches and ignores everything.

### Failure records

A trail is an array of `Failure` records, from `zink.lang`, which every program has:

```java
class Failure {
    string function;    // the function the line is in: `parsePort`, or `Class.method`
    string file;
    i64 line;
    u8 code;            // the status there
    bool unread;        // nothing read it there, which abandoned the call
}
```

- **The trail goes into a `Failure[..~32]`**, declared with the call or already there.
  Another type is "a trail goes into a Failure[..~32], and `t` is Failure[..~8]".
- **With `@@@` alone**, `t.len > 0` says whether anything failed, and `t[^1].code` is what
  `@@` would give.
- **For a call with no value**, the trail has to exist already, as a status variable does in
  `@err = f();`:

```java
class Account {
    i64 balance;

    mut void withdraw(i64 amount) {
        if (amount > balance) {
            return #status.RANGE;
        }
        balance -= amount;
    }
}

void pay(Account& a, i64 amount) {
    a.withdraw(amount);
}

void main() {
    Account acct = new Account(50);
    Failure[..~32] trail = [];
    @@_ @@@trail = pay(acct, 80);
    for (Failure f : trail) {
        println("${f.function} line ${f.line}: ${status.name(f.code)}${f.unread ? ", unread" : ""}");
    }
    println("balance ${acct.balance}");
}
```

```output
Account.withdraw line 6: RANGE
pay line 13: RANGE, unread
balance 50
```

- **The cost**: a failing `return` writes an entry, a few stores, into one fixed buffer of
  32 entries. A `Failure[..~32]` is about 1.5 KB on the caller's stack, and filling it
  copies the trail only when something failed.

## Named codes

Status codes follow Linux's `errno` numbers, so an operating system error passes through
unchanged, with Zinkite's own codes from 192 and a program's own from 224. Each named code
is a constant on `status`:

| Range | Used for |
|---|---|
| 0 | `OK`: success |
| 1 | `FAILED`: a failure with no particular reason, written `#1` |
| 2–191 | Linux's `errno` numbers |
| 192–223 | Zinkite's own codes |
| 224–255 | a program's own codes |

The names the compiler knows, with what `status.message` gives for each:

| Name | Number | `errno` | `status.message` | Also used for |
|---|---|---|---|---|
| `OK` | 0 | | OK | |
| `FAILED` | 1 | | Failed | `#1` |
| `NOT_FOUND` | 2 | `ENOENT` | No such file or directory | |
| `INTERRUPTED` | 4 | `EINTR` | Interrupted system call | |
| `IO` | 5 | `EIO` | Input/output error | |
| `BAD_HANDLE` | 9 | `EBADF` | Bad file descriptor | a closed `File` |
| `AGAIN` | 11 | `EAGAIN` | Resource temporarily unavailable | |
| `NO_MEMORY` | 12 | `ENOMEM` | Cannot allocate memory | |
| `PERMISSION` | 13 | `EACCES` | Permission denied | |
| `BUSY` | 16 | `EBUSY` | Device or resource busy | |
| `EXISTS` | 17 | `EEXIST` | File exists | |
| `CROSS_DEVICE` | 18 | `EXDEV` | Invalid cross-device link | |
| `NOT_DIRECTORY` | 20 | `ENOTDIR` | Not a directory | |
| `IS_DIRECTORY` | 21 | `EISDIR` | Is a directory | |
| `INVALID` | 22 | `EINVAL` | Invalid argument | `i64.parse("abc")`, malformed input |
| `NOT_TERMINAL` | 25 | `ENOTTY` | Inappropriate ioctl for device | |
| `NO_SPACE` | 28 | `ENOSPC` | No space left on device | |
| `BROKEN_PIPE` | 32 | `EPIPE` | Broken pipe | |
| `DOMAIN` | 33 | `EDOM` | Numerical argument out of domain | a division by zero |
| `RANGE` | 34 | `ERANGE` | Numerical result out of range | an overflow, an index, a cast that doesn't fit |
| `NAME_TOO_LONG` | 36 | `ENAMETOOLONG` | File name too long | |
| `NOT_EMPTY` | 39 | `ENOTEMPTY` | Directory not empty | |
| `LOOP` | 40 | `ELOOP` | Too many levels of symbolic links | |
| `UNSUPPORTED` | 95 | `EOPNOTSUPP` | Operation not supported | input using a feature a parser leaves out |
| `TIMEOUT` | 110 | `ETIMEDOUT` | Connection timed out | a queue's `get` with a timeout |
| `EMPTY` | 192 | | Nothing to take | a pop on an empty array |
| `FULL` | 193 | | No room | a push on a full capped array |
| `END` | 194 | | End of input | the end of an iterator; a closed queue |
| `MISSING` | 195 | | No such key or element | |
| `POSITION` | 196 | | Position out of range | a cursor moved past the end |
| `TYPE` | 197 | | Wrong kind of value | `asI64()` on text in a parsed tree |
| `GPU` | 198 | | Error 198 | graphics bindings, not written yet |
| `OUT_OF_DATE` | 199 | | Error 199 | graphics bindings, not written yet |
| `ABANDONED` | 200 | | Abandoned: a failure inside wasn't read | `@a` beside `@@`, for a call that was stopped |

The `errno` name works as well as the readable one, so `status.ENOENT` is
`status.NOT_FOUND`. `status.message(code)` gives the description, which for an `errno`
code is the C library's own (`strerror`, so glibc's on most Linux systems), and
`status.name(code)` the name:

```java
void main() {
    u8[.] codes = [0, 1, 2, 3, 13, 22, 192, 194, 200, 201, 230];
    for (u8 c : codes) {
        println("${c} [${status.name(c)}] ${status.message(c)}");
    }
    println("${status.ENOENT == status.NOT_FOUND} ${status.ABANDONED}");
}
```

```output
0 [OK] OK
1 [FAILED] Failed
2 [NOT_FOUND] No such file or directory
3 [ESRCH] No such process
13 [PERMISSION] Permission denied
22 [INVALID] Invalid argument
192 [EMPTY] Nothing to take
194 [END] End of input
200 [ABANDONED] Abandoned: a failure inside wasn't read
201 [201] Error 201
230 [] Error 230
true 200
```

- **`status.name`** gives the readable name, the C name for an `errno` code without one
  (`ESRCH`), the number for an unnamed code below 224, and `""` for a program's own codes.
- **`status.message`** gives the system's description for codes below 192, Zinkite's own
  for the named codes from 192, and "Error 230" for the rest, `GPU` and `OUT_OF_DATE`
  included.
- **1 is the one exception to `errno`.** It means `EPERM` there, but `#1` is the natural
  way to say "failed", and exit status 1 means general failure too. DESIGN §7.7 reports an
  `EPERM` from the operating system as `PERMISSION` (13), whose description says the same
  thing. Not yet: today it comes through as 1, so `fs.chmod` on a file the program doesn't
  own fails with `FAILED`.
- **A program's own codes** (224–255) have no built-in description, so the program writes
  its own message for them.
- **One status says why a call failed, not the details**, such as a file name. The caller
  writes the message, and it knows the path anyway.

Not yet: DESIGN §7.7 makes every `errno` code available under its C name. Only the C names
of the codes in the table above are known so far: `status.ESRCH` is "`status.ESRCH` isn't a
status code". See [What isn't there yet](15-not-yet.md).

## The exit status

`u8 main` returns the program's exit status, and a failure is often reported as its own
code:

```java
import zink.io.*;

u8 main() {
    string path = "/nonexistent/settings.txt";
    u8[.] text @err = File.readAll(path);
    if (err) {
        errprintln("can't read ${path}: ${status.message(err)}");
        return err;
    }
    print(text);
    return 0;
}
```

```stderr exit=2
can't read /nonexistent/settings.txt: No such file or directory
```

- **A `void main` that returns exits with 0**, and a `u8 main` exits with what it returns,
  0 when it falls off its end.
- **A program stopped by a failure nothing read exits with 1.**
- **`main` can't set a status.** `return #1;` in `main` is an error:

```java
void main() {
    return #1;
}
```

```error
example.zn:2:5: error: `main` can't set a status
help: return the exit status instead: `return 1;` (DESIGN §8.6)
```

- **Common codes don't collide with the shell's own values** (126, 127, and 129–192 for
  death by signals). Zinkite's own codes start at 192, which overlaps only signal 64. Scripts
  that check particular exit codes should still return fixed numbers of their own.

## Statuses in lambdas

A lambda can fail as any function can, with `return #code v;` and `return ## f();` in its
body:

```java
i64 applyAll(func(i64 => i64) f, i64[.] xs) {
    i64 sum = 0;
    for (i64 x : xs) {
        i64 y @err = f(x);
        if (err) {
            return #err sum;
        }
        sum += y;
    }
    return sum;
}

void main() {
    func(i64 => i64) half = (i64 x) => {
        if (x % 2 != 0) {
            return #status.INVALID 0;
        }
        return x / 2;
    };
    i64 a @e1 = half(10);
    i64 b @e2 = half(7);
    println("${a} ${e1} ${b} ${status.name(e2)}");
    i64 s @e3 = applyAll(half, [2, 4, 6]);
    i64 t @e4 = applyAll(half, [2, 3, 6]);
    println("${s} ${e3} ${t} ${status.name(e4)}");
    func(string => i64) parse = (string text) => { return ## i64.parse(text); };
    i64 p @e5 = parse("12");
    i64 q @e6 = parse("twelve");
    println("${p} ${e5} ${q} ${status.name(e6)}");
    i64[..>] xs = [5, 3, 7, 1];
    u8 e7 = @xs.sort((i64 x, i64 y) => {
        if (x == 7 || y == 7) {
            return #status.INVALID false;
        }
        return x < y;
    });
    println("sort: ${status.name(e7)}");
}
```

```output
5 0 0 INVALID
6 0 1 INVALID
12 0 0 INVALID
sort: INVALID
```

- **A call through a `func` value can fail**: `@` reads its status, and one nothing reads
  stops the program at the call. Every `func` can fail, so the type doesn't say so.
- **A lambda passed to a built-in fails that call** with its status, as `sort` above. The
  call stops at once and leaves what it was changing part-way, such as an array half
  sorted.
- **A lambda is a function of its own.** `@` around a call that takes one doesn't catch a
  failure nothing read in the lambda's body, and `@@` does:

```java
i64 sumOf(i64[.] xs, func(i64 => i64) f) {
    i64 sum = 0;
    for (i64 x : xs) {
        sum += f(x);
    }
    return sum;
}

void main() {
    i64[..>] table = [10, 20, 30];
    i64 a @@e1 = sumOf([0, 1, 2], (i64 i) => table[i]);
    println("${a} ${e1}");
    i64 b @@e2 = sumOf([0, 5], (i64 i) => table[i]);
    println("${b} ${status.name(e2)}");
    i64 c @e3 = sumOf([0, 5], (i64 i) => table[i]);
    println("not reached: ${c} ${e3}");
}
```

```output exit=1
60 0
0 RANGE
```

```stderr exit=1
example.zn:15: index 5 is out of range for length 3
```

[Functions and lambdas](08-functions.md) covers lambdas and `func` types.

## Common patterns

### Passing a failure on

A function that can't deal with a failure passes it up, with `##` when nothing else needs
doing, or by reading it and returning `#err` when something does:

```java
import zink.io.*;

i64 countLines(string path) {
    u8[.] text @err = File.readAll(path);
    if (err) {
        return #err 0;
    }
    i64 n = 0;
    for (string _ : text.lines()) {
        n++;
    }
    return n;
}

i64 total(string[.] paths) {
    i64 sum = 0;
    for (string p : paths) {
        i64 n @err = countLines(p);
        if (err) {
            return #err sum;
        }
        sum += n;
    }
    return sum;
}

void main() {
    string path = "/tmp/zink-docs-07-lines.txt";
    File.writeAll(path, "one\ntwo\nthree\n");
    i64 n @err = total([path, "/nonexistent/notes.txt", path]);
    println("${n} lines, then ${status.name(err)}");
    fs.remove(path);
}
```

```output
3 lines, then NOT_FOUND
```

The trail keeps track of where the failure came from. Had nothing read `total`'s status,
the program would have stopped with "`` `countLines` returned it at example.zn:6 ``" and
"`` `total` passed it on at example.zn:20 ``" below the first line.

### Defaults

`@_` keeps the value a failing call gives, which for the library is the zero value. For
another default, read the status and replace the value:

```java
void main() {
    i64 a @_ = i64.parse("abc");
    i64 port @bad = i64.parse("http");
    if (bad) {
        port = 8080;
    }
    i64[.] sizes = [4, 8];
    println("${a} ${port} ${sizes.getOr(5, -1)}");
}
```

```output
0 8080 -1
```

- **Some calls take the default themselves**: `a.getOr(i, fallback)` for an index,
  `x.divOr(d, fallback)` for a division, and the library's `...Or` methods, such as
  `asI64Or` on a parsed value ([The library: data](14-library-data.md)).

### Reporting a failure

A command-line tool reports a failure on stderr with `status.message`, naming what it was
working on, and returns a nonzero exit status:

```java
import zink.io.*;

u8 main() {
    string made = "/tmp/zink-docs-07-stat.txt";
    File.writeAll(made, "twelve bytes");
    string[.] paths = [made, "/nonexistent/a.txt"];
    u8 code = 0;
    for (string p : paths) {
        Stat st @err = fs.stat(p);
        if (err) {
            errprintln("stat: ${p}: ${status.message(err)}");
            code = 1;
            continue;
        }
        println("${p}: ${st.size} bytes");
    }
    fs.remove(made);
    return code;
}
```

```output exit=1
/tmp/zink-docs-07-stat.txt: 12 bytes
```

```stderr exit=1
stat: /nonexistent/a.txt: No such file or directory
```

### A boundary that survives failures

A loop that handles requests, commands or tests one at a time puts `@@` around each, so
one that fails anywhere inside is reported and the loop goes on:

```java
class Command {
    string name;
    i64 arg;
}

i64[..>] memory = [0, 0, 0, 0];

i64 run(Command c) {
    switch (c.name) {
        case "set" => {
            memory[c.arg] = 1;
        }
        case "div" => {
            return 100 / c.arg;
        }
        default => {
            return #status.UNSUPPORTED 0;
        }
    }
    return 0;
}

void main() {
    Command[.] script = [new Command("set", 2), new Command("set", 9), new Command("div", 0),
                         new Command("jump", 1), new Command("div", 5)];
    for (Command& c : script) {
        i64 v @@err = run(c);
        if (err) {
            println("${c.name} ${c.arg}: ${status.message(err)}");
            continue;
        }
        println("${c.name} ${c.arg}: ${v}");
    }
    println("memory ${memory}");
}
```

```output
set 2: 0
set 9: Numerical result out of range
div 0: Numerical argument out of domain
jump 1: Operation not supported
div 5: 20
memory [0, 0, 1, 0]
```

`run` checks nothing: an index out of range or a division by zero fails where it happens,
and the `@@` in the loop catches it. Without the `@@`, the second command would stop the
program.
