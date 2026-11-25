# 10. Programs, packages and building

This chapter covers how a program is laid out in files and how files name each other:
script files, package files, imports and the rules that resolve a name. Then it covers
what a program is compiled with, its settings and its `#if` conditions, what runs before
`main`, and the `zink` command that checks, builds and runs it: its cache, the C compiler
it calls, and the C it generates. It ends with `simd` functions, the standard library's
source, and embedding.

Part of the design in this chapter isn't built yet, most of all programs made of several
files. Each section says what the compiler does today; [What isn't there
yet](15-not-yet.md) lists the rest in one place.

## Script files

A file whose top level has fields or functions is a **script file**, as in Java 25's
compact source files. Most Zinkite programs today are one script file:

```java
#!/usr/bin/env zink

i64 calls = 0;

i64 square(i64 x) {
    calls++;
    return x * x;
}

class Area {
    i64 side;

    i64 size() {
        return square(side);
    }
}

void main() {
    Area a = new Area(4);
    println("${a.size()} after ${calls} call");
}
```

```output
16 after 1 call
```

- **The compiler wraps the file in a hidden class.** Its top-level fields become static
  fields, and its functions static functions, so they work as statics do everywhere
  ([Classes, interfaces and generics](06-classes.md)). `calls` above is a static, which any
  function may change, `Area.size` included.
- **A script file can declare classes, interfaces and enums** beside its functions. They
  are visible only in that file.
- **It is in the unnamed package.** It has no `package` line, and nothing can import it.
  It can import packages, the standard library's included.
- **The entry point is a top-level `main`** (see [`main` and the exit
  status](#main-and-the-exit-status)).
- **`#!` on the first line** is skipped by the lexer, so a script with the executable bit
  set runs directly. `#!/usr/bin/env zink` calls `zink FILE`, which is `zink run FILE`:

```
$ chmod +x greet.zn
$ ./greet.zn Linus
hello, Linus
```

The design gives reasons for script files in [DESIGN §8.6](../DESIGN.md#86-script-files-decided).
Two of its rules aren't checked yet: the compiler accepts a `package` line in a script
file, and `public`, `private` or `static` on its top-level declarations, and ignores them.

## Package files

A file whose top level holds only classes, roots, interfaces and enums is a **package
file**. Its `package` line names its package, and it holds one public top-level
declaration named after the file, plus any non-public ones it needs:

```java
// src/com/example/util/Counter.zn
package com.example.util;

public class Counter {
    public i64 n;

    public mut void bump() {
        n++;
    }
}

class Step {
    i64 by;
}
```

- **The public declaration is the file's interface**: `Counter.zn` holds `public class
  Counter`. Other packages can import it.
- **A declaration without `public` is package-private**: `Step` is visible to every file
  of `com.example.util` and to no other ([Classes, interfaces and generics](06-classes.md)
  has the visibility rules).
- **File order** is fixed: the shebang, settings (entry file only), `package`, imports,
  then declarations.
- **`zink.*` is reserved for the standard library**, and **`vendor.*`** for bindings to
  third-party C libraries that would ship beside it. A program's own package can't be
  in `zink`:

```
$ zink check Tool.zn
Tool.zn:1:9: error: `zink.*` is reserved for the standard library (DESIGN §8.4)
    package zink.tools;
            ^^^^^^^^^^
1 error and 0 warnings in 1 file
```

Today a package file can be checked, but not run. `zink check` type-checks it on its own,
as a program with no entry point. The design's entry point for a package, a `static main`
in its public class, isn't one yet: the compiler generates no C `main` for it, and
`zink run` fails when the C compiler links the program:

```
$ zink run App.zn
zink: the C compiler failed on the generated code, which is a bug in zink:
/usr/bin/ld: /lib/x86_64-linux-gnu/Scrt1.o: in function `_start':
(.text+0x1b): undefined reference to `main'
clang: error: linker command failed with exit code 1 (use -v to see invocation)
```

The rules on the file itself aren't checked yet either: a `package` line that doesn't
match the file's directory, a second public class, or a public class not named after the
file are accepted.

## Packages and directories

In the design a package is a directory, as in Java, and each file's `package` line
matches its path ([DESIGN §8.4](../DESIGN.md#84-packages-and-files-decided)):

```
src/
  com/example/app/Main.zn        package com.example.app;
  com/example/util/Counter.zn    package com.example.util;
```

- **The source root is inferred from the entry file**: `zink run
  src/com/example/app/Main.zn`, whose `package` line is `com.example.app`, gives the root
  `src/`. Libraries outside it would come from a `--source-path` flag.
- **Circular dependencies between packages are allowed**, since the compiler sees the
  whole program at once.

None of this is built yet. **A program is one file today**, plus the standard library:

- `zink run`, `zink build` and `zink c` read one file;
- `zink check a.zn b.zn` checks each file as a program of its own, so neither sees the
  other's declarations;
- there is no `--source-path`;
- an import of a program's own package is accepted, and brings in nothing.

So the layout above doesn't compile:

```
$ zink check src/com/example/util/Counter.zn src/com/example/app/Main.zn
src/com/example/app/Main.zn:7:9: error: unknown type `Counter`
            Counter c = new Counter(0);
            ^^^^^^^
src/com/example/app/Main.zn:7:25: error: unknown type `Counter`
            Counter c = new Counter(0);
                            ^^^^^^^
2 errors and 0 warnings in 2 files
```

Until programs of several files are built, write a program as one script file. Its
classes can be as many as it needs, and package-private visibility then means the file.
The examples in `examples/` are each one file, the largest about 1,800 lines.

## Imports

Imports come after the `package` line, if any, and before the declarations. There are
three forms ([DESIGN §8.5](../DESIGN.md#85-imports-decided)):

| Import | What the file can then name |
|---|---|
| `import zink.io.path;` | `path` |
| `import zink.time.Duration as Span;` | `Span`, and not `Duration` |
| `import zink.io.*;` | every public declaration of `zink.io` |

```java
import zink.io.path;
import zink.time.Duration as Span;

void main() {
    println(path.join("logs", "today.txt"));
    Span wait = Span.ms(1500);
    println("${wait.toMs()} ms");
}
```

```output
logs/today.txt
1500 ms
```

**`as` renames a declaration for this file only.** The original name isn't imported, so
after `import zink.time.Duration as Span;` a bare `Duration` means whatever it would mean
without that line:

```java
import zink.time.Duration as Span;

void main() {
    Duration d = Duration.ms(3);
    println("${d.toMs()}");
}
```

```error
example.zn:4:5: error: unknown type `Duration`
```

**A wildcard imports every public declaration of a package:**

```java
import zink.io.*;

void main() {
    string file = "/var/log/app/today.log";
    println("${path.base(file)} ${path.ext(file)} ${path.dir(file)}");
    println("${fs.exists("/")}");
}
```

```output
today.log .log /var/log/app
true
```

**A full name works in a type** when the program has the package: `zink.time.Duration d
= ...;` needs no import of `Duration`, since every program has `zink.time`. (A full name in
another `zink.*` package the program doesn't have isn't checked yet, and fails only when the
C is generated.) In an expression a full name doesn't work yet, so
`zink.io.path.join("a", "b")` is an error, ``unknown name `zink` ``. Import the class
instead.

**An import of a `zink.*` package that isn't written yet** is a warning. The names the
file uses from it aren't checked, and a program that uses one doesn't compile to C:

```java
import zink.toml.*;

void main() {
    println("no TOML yet");
}
```

```warning
example.zn:1:1: warning: `zink.toml` isn't written yet, so the names this file uses from it aren't type-checked
```

```output
no TOML yet
```

The design has more rules on imports than the compiler checks today. Until it does:

- **an import of a package that doesn't exist** outside `zink.*`, such as `import
  com.example.util.Counter;` in a one-file program, or of a name a library package doesn't
  have, such as `import zink.io.Nope;`, is accepted and brings in nothing;
- **two imports of one name**, and an import that clashes with a declaration of the file,
  aren't errors. The first import of a name wins, even when it's a wildcard and a later
  line imports the same name explicitly, where the design says the explicit import wins;
- **unused imports** draw no warning;
- **a non-public class** of another package can be imported and named as a type.

Static imports, and aliases for whole packages, are undecided (DESIGN §17).

## How names are resolved

A name in an expression or a type is looked up in this order:

1. **Locals and parameters**, innermost first. A local hides any class with the same
   name, imported or declared, inside its scope, as in Java.
2. **The file's own declarations**: its classes, and in a script file its top-level
   fields and functions.
3. **Imports.**
4. **`zink.lang`**, which every file imports.

So a local `string path` hides the class `zink.io.path` that `import zink.io.*` brings in,
and `path.len` is the string's length:

```java
import zink.io.*;

void main() {
    string path = "/tmp/notes.txt";
    println("${path.len}");
}
```

```output
14
```

A declaration of the program's own hides the `zink.lang` one with the same name wherever
it is visible, as a class of your own hides `java.lang`'s in Java. It hides every overload:
with this `max` in the file, `max(1, 2)` is an error, `` `max` takes 1 argument, not 2 ``.

```java
i64 max(i64[:] xs) {
    i64 best = xs[0];
    for (i64 x : xs) {
        if (x > best) {
            best = x;
        }
    }
    return best;
}

void main() {
    i64[.] sizes = [3, 9, 4];
    println("${max(sizes)}");
}
```

```output
9
```

**`zink.lang` is imported into every file.** It holds `print`, `println`, `errprint` and
`errprintln`, the streams `stdin`, `stdout` and `stderr`, `status`, `env`, `math`, `min`,
`max`, `clamp` and `abs`, the methods on numbers, text and arrays, `hashmap`, `hashset`,
`pool` and `Handle`, and `Iterator<T>` with the library's iterators. [The library: files,
processes, time and the command line](13-library-system.md) describes them.

**`zink.io` is in every program**, since the standard streams are `File`s, but its names
aren't imported into every file. `stdout.println(...)` works anywhere; naming the type
`File`, or `path` and `fs`, needs an import:

```java
void main() {
    File out = stdout;
    out.println("hello");
}
```

```error
example.zn:2:5: error: unknown type `File`
```

The same holds for the built-in `queue<T>`, which needs `import zink.thread.*;`
([Threads](11-threads.md)).

## Program-wide settings

A **setting** is a directive line, `#name value`, that applies to the whole program
([DESIGN §8.2](../DESIGN.md#82-program-wide-settings-decided)):

```java
#!/usr/bin/env zink
#max_memory_mb 64

void main() {
    println("settings come first");
}
```

```output
settings come first
```

A `#` that is the first thing on a line and is followed by a letter starts a directive,
unless the token before it is `return`, where `#` writes a status ([Errors and
statuses](07-errors.md)).

The design has two settings:

| Setting | Value | Default | Meaning |
|---|---|---|---|
| `#max_memory_mb` | integer | none | a ceiling on the memory committed to data stacks; going past it stops the program |
| `#data_stack_gb` | integer | 64 | address space reserved per data stack, which isn't memory |

and these rules for them:

- they come after the shebang and before the `package` line, imports and declarations;
- they go only in the file that holds the entry point, so a library can't change how a
  program runs;
- an unknown setting is an error, so a typo isn't ignored, and each setting appears once;
- a build flag, such as `--max_memory_mb=512`, overrides a setting, and an embedding host
  overrides both.

What the compiler does today is less. It checks a setting's form, one value, a word or an
integer:

```java
#max_memory_mb
void main() {
    println("no value");
}
```

```error
example.zn:1:1: error: `#max_memory_mb` takes one value: a word or an integer
```

and its place: a setting after an import or a declaration is a syntax error, ``expected a
type, found `#max_memory_mb` ``.

**It does nothing else with them yet.** Neither setting has an effect: every data stack
reserves 8 GB of address space, whatever `#data_stack_gb` says, and nothing limits the
memory committed. An unknown name, such as `#max_memory 64`, and a repeated setting are
accepted, and there are no build flags for them. How `#max_memory_mb` would be enforced
without a check on every growth is undecided (DESIGN §17).

## Conditional compilation

`#if`, `#else` and `#endif` choose code at compile time ([DESIGN
§8.3](../DESIGN.md#83-if-decided)). They go around settings, top-level declarations, the
members of a class or interface, and statements:

```java
#if os(linux)
string platform() {
    return "Linux";
}
#else
string platform() {
    return "elsewhere";
}
#endif

class Counter {
    i64 n;
    #if debug
    i64 checks;
    #endif
}

void main() {
    println(platform());
    #if debug
    println("a debug build");
    #else
    println("not a debug build");
    #endif
    #if arch(x86_64) && !color
    println("x86-64, no color");
    #endif
    #if os(windows)
    useTheWindowsConsole();
    #endif
    Counter c = new Counter(0);
    println("${c.n}");
}
```

```output
Linux
not a debug build
x86-64, no color
0
```

- **A condition** is made of names, names with one argument such as `os(linux)`, `!`,
  `&&`, `||` and parentheses. Anything else is an error, `` an `#if` condition uses
  names, `!`, `&&`, `||` and parentheses ``.
- **Each branch is whole declarations or statements**, and both branches must parse. Only
  the active one is type-checked, so `useTheWindowsConsole()` above, which doesn't exist,
  is no error, while a typo in it would be.
- **There is no `#define`** and no text substitution, so `#if` can't grow into the C
  preprocessor.
- **`#if` can wrap settings**, which gives per-build settings.

The names that are true, today:

| Condition | Value |
|---|---|
| `os(linux)` | true |
| `arch(x86_64)` | true |
| `release` | true |
| `debug` | false |
| any other `os(...)` or `arch(...)`, such as `arch(aarch64)` | false |
| any other name, such as `color` | false |

The compiler doesn't look at the machine: it compiles for Linux on x86-64 wherever it
runs, and `zink run`, `zink run -O` and `zink build` all see the table above. The design's
build flags, a debug build that makes `debug` true and `--define color` that makes `color`
true, aren't built, so there is no way to change a condition from the command line yet.
A name that nothing defines is false, with no warning.

**`#elif` is undecided** (GRAMMAR §8):

```java
void main() {
    #if os(linux)
    println("Linux");
    #elif os(macos)
    println("macOS");
    #endif
}
```

```error
example.zn:4:5: error: `#elif` is undecided (GRAMMAR §8); nest an `#if` inside the `#else`
```

Nest an `#if` inside the `#else` instead:

```java
void main() {
    #if os(macos)
    println("macOS");
    #else
    #if os(linux)
    println("Linux");
    #else
    println("elsewhere");
    #endif
    #endif
}
```

```output
Linux
```

An inactive branch that doesn't parse is an error like any other. Here the missing `;`
makes the `#endif` part of a statement, so the `#if` has none:

```java
void main() {
    #if os(windows)
    println("Windows")
    #endif
}
```

```error
example.zn:2:5: error: this `#if` has no `#endif`
example.zn:4:5: error: expected `;` after the statement, found `#endif`
```

## Static initialization

Every static field, a top-level field of a script file included, is initialized once,
before `main` runs. An initializer can be any expression: it can call the program's
functions, which run before `main`, and build arrays. A growable static gets a data stack
of its own. A static without an initializer starts as its zero value, and a growable array,
`hashmap`, `hashset` or `pool` without one starts empty:

```java
i64[..>] primes = primesBelow(30);
hashmap<string, i64> seen = [];

i64[..>] primesBelow(i64 n) {
    i64[..>] out = [];
    for (i64 i = 2; i < n; i++) {
        bool prime = true;
        for (i64 d = 2; d * d <= i; d++) {
            if (i % d == 0) {
                prime = false;
            }
        }
        if (prime) {
            out +> i;
        }
    }
    return out;
}

void main() {
    primes +> 31;
    seen.put("main", 1);
    println("${primes}");
    println("${seen.len}");
}
```

```output
[2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31]
1
```

Static values live for the whole run. Nothing runs for them at exit, except that the
runtime writes out and closes any files still open.

**The order is the order they are written in the file today**, top-level fields and the
statics of each class alike, and the library's statics after the program's. The design
orders them by dependency instead: if the initializer of `A.x` reads `B.y`, directly or
through a function it calls, `B.y` comes first, and initializers that depend on each other
in a loop are an error that lists the loop ([DESIGN
§8.7](../DESIGN.md#87-static-initialization-decided)). Until that is built, an initializer
that reads a static written after it reads that static's zero value:

```java
i64 total = width * height;
i64 width = 80;
i64 height = 24;

void main() {
    println("${total} ${width * height}");
}
```

```output
0 1920
```

So write each static after the ones its initializer reads. Two initializers that read
each other aren't an error yet either; the first reads the second's zero value.

**A failure in an initializer that nothing reads stops the program before `main`**, with
exit status 1, as a failure in `main` would ([Errors and statuses](07-errors.md)):

```java
i64 limit = i64.parse("12x");

void main() {
    println("never printed");
}
```

```stderr exit=1
example.zn:1: `parse` failed with INVALID, and nothing read its status
```

A field's initializer can't capture a status with `@` as a local's declaration can:
`i64 limit @bad = i64.parse(text);` at the top level is a syntax error. Read the status in
a function, and return a value:

```java
i64 limit = parseOr("12x", 100);

i64 parseOr(string text, i64 fallback) {
    i64 n @bad = i64.parse(text);
    if (bad) {
        return fallback;
    }
    return n;
}

void main() {
    println("${limit}");
}
```

```output
100
```

There is no lazy initialization on first use, so startup cost is predictable. Static
initializer blocks, `static { ... }`, are undecided:

```java
class Table {
    static i64 size;
    static {
        size = 64;
    }
}

void main() {
    println("${Table.size}");
}
```

```error
example.zn:3:5: error: static initializer blocks are undecided (GRAMMAR §8)
```

## `main` and the exit status

A program starts at a top-level `main` in a script file, after the statics are
initialized ([The basics](02-basics.md) introduces it):

| Declaration | Exit status |
|---|---|
| `void main()`, `void main(string[.] args)` | 0 |
| `u8 main()`, `u8 main(string[.] args)` | what it returns, and 0 if it falls off its end |

- **`args` holds the arguments after the program's name**: `zink run tool.zn -v x` gives
  `["-v", "x"]`. The name is `process.name()` in `zink.os`; under `zink run` it is the
  path of the cached executable.
- **The status is a `u8`**, since Linux keeps only the low 8 bits, so `return 256;` is an
  error rather than a success.
- **`main` can't set a status**: `return #1;` in `main` is an error that suggests
  `return 1;`.
- **There is no `exit`.** Only returning from `main` ends a program. A function deep in it
  fails with a status, and its callers pass it up.
- **A failure nothing reads stops the program with exit status 1**, wherever it happens
  ([Errors and statuses](07-errors.md)).

```java
u8 main(string[.] args) {
    if (args.len == 0) {
        errprintln("usage: greet NAME");
        return 2;
    }
    println("hello, ${args[0]}");
}
```

```stderr exit=2
usage: greet NAME
```

Any other parameters or result type for `main` are errors, such as `` `main` returns
`void`, or `u8` for the exit status (DESIGN §8.6) ``.

## The `zink` command

The compiler is one command, `zink` ([DESIGN §14.4](../DESIGN.md#144-command-line-tool-decided)):

| Command | What it does |
|---|---|
| `zink run [-O] FILE [args...]` | compile into the cache if needed, then run the program with `args` |
| `zink FILE [args...]` | the same as `zink run`; what `#!/usr/bin/env zink` calls |
| `zink build FILE -o OUT` | compile to an executable at `OUT`, with the C compiler at `-O2` |
| `zink check FILE...` | parse and type-check each file as a program of its own |
| `zink c FILE` | print the C generated for a program |
| `zink parse [--tree] FILE...` | parse each file and report syntax errors; `--tree` prints the syntax tree |
| `zink lex FILE` | print the tokens of a file |
| `zink cache clean` | empty the cache |
| `zink`, `zink -h`, `zink --help` | print the list of commands |

### `zink run`

`zink run` checks the program, generates its C, and looks for an executable built from
that C in the cache. If there is none, it compiles the C at `-O0` and stores the result.
Then it runs the program in its own place, with `exec`: the program keeps `zink`'s process
id, standard streams and signals, and its exit status is the command's. Pipes, `$?` and
Ctrl-C behave as if the executable were run directly.

```
$ zink run greet.zn Ada Grace
hello, Ada
hello, Grace
$ zink run -O greet.zn -O
hello, -O
```

- **`-O` must come right after `run`.** It compiles at `-O2`, for scripts with hot loops.
  Everything after the file goes to the program unchanged, `-O` included.
- **`zink FILE`** works when `FILE` ends in `.zn` or is an existing file, and is what a
  shebang line calls. It takes no `-O`; a script that should run at `-O2` starts with
  `#!/usr/bin/env -S zink run -O`.
- **The first run after an edit pays for compiling**: about a quarter of a second for a
  small program, most of it the C compiler. A later run of the same program checks and
  generates its C again, hashes it, and starts the cached executable.
- **Errors stop it before anything runs**, after the compiler's messages and a line such
  as `1 error in greet.zn`. Warnings are printed, and the program runs.

### `zink build`

`zink build FILE -o OUT` writes an executable to `OUT`, compiled at `-O2`, and prints
nothing when it succeeds. `-o OUT` may come before or after the file. The C is written
next to `OUT` as a hidden temporary file, `.zink-PID-N.c`, and removed afterwards.

```
$ zink build greet.zn -o greet
$ ./greet Ken
hello, Ken
```

The executable is linked dynamically against the C library and `libm`, as the C compiler
links by default; a program that prints `hello` is about 17 KB.

### `zink check`, `zink c`, `zink parse` and `zink lex`

`zink check` parses and type-checks each file it is given, as a program of its own, and
ends with a summary on stderr:

```
$ zink check greet.zn
checked 1 file with no errors
$ zink check notoml.zn
notoml.zn:1:1: warning: `zink.toml` isn't written yet, so the names this file uses from it aren't type-checked
    import zink.toml.*;
    ^^^^^^^^^^^^^^^^^^^
checked 1 file with no errors and 1 warning
```

It runs every check `zink run` does except generating C, so a few things it accepts don't
compile yet: those are errors that say ``the C backend doesn't support ... yet``, with the
help line `` `zink check` accepts it; code generation for it comes in a later milestone ``.
[What isn't there yet](15-not-yet.md) lists them.

`zink c FILE` prints the C that `zink run` and `zink build` compile, described below.
`zink parse` reports syntax errors only, and with `--tree` prints the syntax tree, one
statement per line:

```
$ zink parse --tree tiny.zn
field i64 total
void main()
  (for (<= i 3))
    (+= total i)
  (call println "total ${total}")
parsed 1 file with no errors
```

`zink lex` prints each token with its line and column, as `2:1	Setting { name:
"max_memory_mb", value: "64" }` for a setting line.

### Messages and exit statuses

Each message names the file, line and column, says `error` or `warning`, and shows the
line with a caret under the part it means. Many have a `help:` line after it:

```
$ zink run err.zn
err.zn:2:13: error: expected i64, found string
        i64 x = "text";
                ^^^^^^
1 error in err.zn
```

| Exit status | When |
|---|---|
| 0 | success: no errors, warnings or not |
| 1 | errors in the program, a file that can't be read, no C compiler, or a C compiler that failed |
| 2 | a command used wrongly, such as `zink build FILE` without `-o`, or an unknown command |
| the program's | `zink run` and `zink FILE`, once the program starts |

A usage error prints the command's form, such as `usage: zink build FILE -o OUT`. An
unknown command prints `` zink: unknown command `test` `` and the list of commands.

## The cache

`zink run` keeps its executables in `$XDG_CACHE_HOME/zink/`, or `~/.cache/zink/` when
`XDG_CACHE_HOME` isn't set.

- **The directory is created with mode 0700, and must be the user's own.** If another user
  owns it, or its group or other users can write to it, `zink` refuses to run anything
  from it, since a planted executable would run as the user. The message names the
  directory: `` zink: DIR isn't owned by you, or others can write to it, so zink won't run
  binaries from it ``.
- **An executable is cached under a hash** of the generated C, the C compiler, its flags
  and the version of `zink`. A changed program, `-O`, another `$CC` or a new `zink` builds
  again, and an executable from an older compiler is never run. The C names the source
  file as it was given, so `zink run greet.zn` and `./greet.zn` are cached separately.
- **It is written under a temporary name and renamed** into place, so two runs at once never
  see half an executable.
- **What clang reads from C headers is cached too**, for `extern import` ([Calling
  C](12-calling-c.md)), in files named `h-` and a hash: keyed by the headers' names, the
  clang binary and the version of the header reader, and kept while every file clang read
  is unchanged. A cached reading checks those files' sizes and times and starts no
  process. `zink check` uses and fills this cache as well.
- **`zink cache clean`** removes every file in the directory, header readings included,
  and says how many: `removed 3 cached programs`.

## The C compiler

`zink run` and `zink build` compile the generated C with the first of these that it finds
([DESIGN §14.2](../DESIGN.md#142-what-emitting-c-requires)):

1. `$CC`, when it is set and not empty;
2. `clang`;
3. `gcc`;
4. `cc`.

Clang comes first because its code was as fast as gcc's or faster on Zinkite programs,
and it compiles faster at `-O0`. With none of them, `zink` stops with `zink: no C compiler
found; install gcc or clang, or set $CC`.

The command lines, as `zink` runs them with clang:

```
clang -std=gnu11 -w -fwrapv -fno-strict-aliasing -ffp-contract=off -O0 -o EXE FILE.c -lm
clang -std=gnu11 -w -fwrapv -fno-strict-aliasing -ffp-contract=off -O2 -mbranches-within-32B-boundaries -o EXE FILE.c -lm
```

- **`-O0` for `zink run`, `-O2` for `zink build` and `zink run -O`.** At `-O0` a program
  starts sooner and runs slower in hot loops.
- **`-fwrapv` and `-fno-strict-aliasing` are safety nets.** The generated C has no
  undefined behavior to begin with: wrapping arithmetic goes through unsigned types, and
  the checked operators through `__builtin_add_overflow` and its siblings.
- **`-ffp-contract=off`** keeps a multiply and an add two roundings, so a program prints
  the same digits wherever it runs, `simd` functions included.
- **`-w`** hides the C compiler's warnings, which are about the generated code, not the
  program.
- **`-lm`** is always linked, and each `extern link` library adds its `-l` ([Calling
  C](12-calling-c.md)).
- **An optimized build on x86-64 keeps jumps off 32-byte boundaries** (next section).

**Reading C headers always needs clang**, which can list what a header declares; it is
`$CC` when that is a clang, and otherwise `clang` on the path. The program is then built
with whichever compiler the list above finds.

A failure of the C compiler is a bug in `zink`, since the C it generates should always
compile; the message says so and shows the C compiler's first 30 lines.

## How a program becomes C

`zink c FILE` shows what the other commands compile ([DESIGN
§12.5](../DESIGN.md#125-backends-and-executables-decided)):

- **One C file for the whole program**, starting with `/* Generated by zink. */` and the
  runtime, `compiler/runtime/zink_rt.h`, about 3,300 lines of C: data stacks, output,
  failures, checked arithmetic, text, hashing and the C layer of the library. Then come
  the program's structs, its statics and their initializer, its functions, and a C `main`
  that starts the runtime, initializes the statics and calls the program's `main`.
- **`#line` directives** before each statement point C compiler messages and debuggers at
  the `.zn` lines.
- **Generics are instantiated on demand** ([DESIGN §10](../DESIGN.md#10-generics)): each
  generic function, and each method of a generic class, is generated once for each set of
  type arguments it is used with, and not at all if it isn't used. In a type parameter
  bounded by an interface, the methods are called directly.
- **Only the library functions a program calls are generated**, the way instantiations
  are. A program that prints `hi` has none: its C is the runtime, the library's struct
  definitions, a few of the library's statics, the program's `main` and the C `main` that
  calls it.
- **Small functions are `static inline`**, so a check inside one doesn't keep the C
  compiler from inlining it.
- **A counted `for` loop that indexes arrays is generated twice** when a test before it
  can show every index in range: once without index checks, which runs when the test
  passes, and once with them ([DESIGN §14.2](../DESIGN.md#142-what-emitting-c-requires)).
- **A recursive function's base case is copied to its calls to itself**, so a tree walk
  makes about half as many calls (also DESIGN §14.2).
- **A program that imports `zink.thread`** is compiled with `ZR_THREADS`, which makes the
  runtime's state and the program's statics per-thread ([Threads](11-threads.md)). Any
  other program compiles to the same C as before threads existed.

**Jumps are kept off 32-byte boundaries on x86-64** in optimized builds: `zink build` and
`zink run -O` pass `-mbranches-within-32B-boundaries` to clang, or
`-Wa,-mbranches-within-32B-boundaries` to gcc for its assembler, whichever the compiler
takes, tested once per run of `zink`. On Intel's Skylake through Cascade Lake and Comet
Lake, the microcode for the JCC erratum keeps the decoded-instruction cache from holding a
jump that crosses or ends on such a boundary, so a hot loop with one decodes every
instruction again. On the benchmark machine it made graph 12% faster and calc 9%, and
seven benchmarks 1–2% slower from the padding. Other CPUs don't need it, but an
executable doesn't know where it will run.

## `simd` functions

`simd` before a function or a method compiles it three times: for baseline x86-64, for
AVX2 and for AVX-512 ([DESIGN §12.8](../DESIGN.md#128-simd-functions-decided-2026-11-23)).
The first call picks the widest version the CPU and its operating system support, so the
C compiler can vectorize a hot loop with the widest vectors the machine has, while the rest
of the program runs on every x86-64:

```java
simd f64 dot(f64[:] a, f64[:] b) {
    f64 sum = 0.0;
    for (i64 i = 0; i < a.len; i++) {
        sum += a[i] * b[i];
    }
    return sum;
}

void main() {
    f64[..>] a = [];
    f64[..>] b = [];
    for (i64 i = 0; i < 1000; i++) {
        a +> (f64) i;
        b +> 0.5;
    }
    println("${dot(a, b)}");
}
```

```output
249750.0
```

- **The AVX2 version** is built for AVX2 with FMA, BMI1, BMI2 and POPCNT, about x86-64-v3;
  **the AVX-512 version** adds AVX-512 F, BW, CD, DQ and VL, about x86-64-v4. The
  baseline, under the function's own name, asks the CPU once and calls the widest, so a
  caller sees one function.
- **Every version gives the same results.** Floats are never contracted: `a * x + y`
  rounds the multiply and then the add in the versions with FMA too. Integer arithmetic and
  index checks are the language's everywhere.
- **The functions it calls are their own**, called as usual, though the C compiler may
  inline small ones into each version. A call to a `simd` function goes through the choice
  each time, a load and a branch, so `simd` belongs on the function that holds the loop,
  not on a small one called inside it.
- **Only clang on x86-64 builds the wider versions.** With gcc, or on another CPU, the
  baseline is all there is, and the program gives the same results.
- **It goes on functions and methods with a body**, generic ones included, and not on
  fields, constructors, `extern` functions or `main`:

```java
simd void main() {
    println("hi");
}
```

```error
example.zn:1:11: error: `simd` can't go on `main`: put the loops in a function of their own and mark that one (DESIGN §12.8)
```

`simd` is a keyword only as a modifier, before a function's return type or type
parameters, so it can still name a variable or a class: `i64 simd = 3;` is a static named
`simd`.

## The standard library is Zinkite

The standard library is Zinkite source, in `compiler/lib/zink/`, built into the `zink`
binary ([DESIGN §12.7](../DESIGN.md#127-the-standard-library-is-zinkite-decided)). Its
files are the best examples of Zinkite there are, and reading them is the way to see how a
library function works: `compiler/lib/zink/io/path.zn` is `path`, and
`compiler/lib/zink/json/JsonReader.zn` is the JSON reader.

- **A program gets the packages its file imports**, the ones those import in turn, and
  `zink.lang`, `zink.io` and the `zink.time` that `zink.io` uses, which every program has.
  They are checked and compiled with the program, as more files with names such as
  `<zink>/zink/io/File.zn`.
- **`zink.lang` is partly built into the compiler**: output, `status`, `math`, and most
  methods on numbers, text and arrays. The rest of it is Zinkite: `env`, `Iterator<T>`,
  the iterators over text and arrays, and text methods such as `split` and `repeat`.
- **The library's warnings aren't shown.** An error in a library file is a bug in `zink`,
  reported on the program's file as `the standard library didn't compile, which is a bug in
  zink:` and the library file, line and message.
- **Its C layer is the runtime's `zr_` functions**, declared as `extern`s that take
  Zinkite's own representations, such as `extern("zr_io_open") i64 cOpen(string path, i32
  how);`. Only the library declares them; a program that tries gets `` `zr_` functions are
  the runtime's, which only the standard library calls ``.

Which packages are written, and what in them isn't, is in [The library: files, processes,
time and the command line](13-library-system.md), [The library: data](14-library-data.md)
and [What isn't there yet](15-not-yet.md).

## Embedding

The design means Zinkite to run inside other programs too, as a scripting language a host
loads. How is undecided ([DESIGN §13](../DESIGN.md#13-embedding-in-other-programs-open)):
TinyCC inside the host, compiling scripts as it loads them; precompiled `.so` plugins that
`zink build` would make, loaded through a C ABI; a bytecode VM over the same typed IR,
roughly 10 to 50 times slower than native code; or none, since command-line tools don't
need it.

Whichever is chosen, the host would set the program's settings through its API, overriding
directives, and a script without `main` would have its statics initialized when the host
loads it. What a host may hold into a script's data, and for how long, is an open question
of the reference rules (`REFERENCES.md` §12).

None of it is built: `zink` makes executables only, and the runtime has no host API.
