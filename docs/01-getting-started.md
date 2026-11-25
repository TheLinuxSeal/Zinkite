# 1. Getting started

This chapter covers what `zink` needs, building and installing it, the first programs,
scripts that run with `#!`, every subcommand of `zink`, the cache that makes a script's
second run fast, and which C compiler builds a program. It ends with a short tour of a
larger program and a map of the rest of the documentation.

## What you need

- **64-bit Linux**, on x86-64 or AArch64. A program keeps its data on *data stacks*, each a
  large reservation of address space that the kernel backs with memory only as it is
  touched, so Zinkite needs virtual memory ([Memory and references](05-memory.md),
  [DESIGN §3.1](../DESIGN.md#31-data-stacks-decided)).
- **Rust 1.88 or later**, to build the compiler.
- **A C compiler.** `zink` turns a program into C and builds it with the system's C
  compiler. clang is preferred, and is needed for `extern import`, which reads C headers
  with it ([Calling C](12-calling-c.md)); gcc works for everything else.

The standard library and the runtime are built into the `zink` binary, so there is nothing
else to install.

## Building and installing

```
git clone https://github.com/TheLinuxSeal/Zinkite
cd Zinkite/compiler
cargo build --release
cargo test
```

The compiler is then `compiler/target/release/zink`. To put it on the path, run this from
the repository's root:

```
cargo install --path compiler
```

which installs `zink` in `~/.cargo/bin`.

### With Nix

The repository is a flake. Its `zink` brings its own clang, which it uses when the path has
no clang and `$CC` isn't set. To run a program without installing anything:

```
nix run github:TheLinuxSeal/Zinkite -- hello.zn
```

To install it, add the flake to a NixOS or Home Manager flake:

```nix
{
  inputs.zink.url = "github:TheLinuxSeal/Zinkite";
  inputs.zink.inputs.nixpkgs.follows = "nixpkgs";

  outputs = { nixpkgs, zink, ... }: {
    nixosConfigurations.HOST = nixpkgs.lib.nixosSystem {
      modules = [
        ({ pkgs, ... }: {
          environment.systemPackages = [ zink.packages.${pkgs.stdenv.hostPlatform.system}.default ];
        })
      ];
    };
  };
}
```

- With Home Manager, put the same package in `home.packages`.
- The flake also has an overlay, `zink.overlays.default`, which adds `pkgs.zink`.
- `nix flake update zink` picks up a newer compiler.
- The nixpkgs it follows needs Rust 1.88 or later, as in NixOS 26.05.
- The tests run in `nix flake check`, not when the package is built.

## The first program

Source files end in `.zn`. Put this in `hello.zn`:

```java
void main() {
    println("hello, world");
}
```

```output
hello, world
```

and run it:

```
$ zink run hello.zn
hello, world
```

- **A file doesn't need a class.** A file whose top level has functions and fields, and no
  `package` line, is a *script file*: the compiler puts its functions and fields in a
  hidden class as static members ([DESIGN §8.6](../DESIGN.md#86-script-files-decided)).
  A script file can declare classes too, as the tour below does.
- **`main` is where the program starts**, and returning from it is the only way a program
  ends. There is no `exit` function.
- **`println` prints text and a newline**, and `print` prints text alone. Both take a
  `string`; values go into the text with `${...}`, as in `"${count} files"`
  ([Text](03-text.md)).
- Functions can come in any order: `main` can call one declared after it.

## Arguments and the exit status

`main` can take the command-line arguments, as `string[.] args`, a frozen array of views of
them. `args[0]` is the first argument, not the program's name. A `u8 main` returns the
exit status:

```java
u8 main(string[.] args) {
    if (args.len == 0) {
        errprintln("usage: greet NAME...");
        return 2;
    }
    for (string name : args) {
        println("hello, ${name}");
    }
    return 0;
}
```

```
$ zink run greet.zn ada bo
hello, ada
hello, bo
$ zink run greet.zn
usage: greet NAME...
$ echo $?
2
```

- **`void main()` and `void main(string[.] args)`** always exit with 0.
- **`u8 main()` and `u8 main(string[.] args)`** exit with the value they return. Falling
  off the end of one returns 0, as in C.
- **The status is a `u8`** because Linux keeps only the low 8 bits of an exit status: with
  a wider type, `return 256;` would reach the shell as 0, which means success.
- **A failure that nothing reads stops the program** with a message naming its line, and
  exit status 1 ([Errors and statuses](07-errors.md)).
- `errprintln` and `errprint` write to standard error.

A program whose `main` returns 3:

```java
u8 main() {
    println("checking");
    return 3;
}
```

```output exit=3
checking
```

`main` itself is covered in [The basics](02-basics.md#main).

## Scripts with `#!`

`zink FILE` does what `zink run FILE` does, so a file whose first line is
`#!/usr/bin/env zink` runs as a command once it is executable. The compiler ignores a `#!`
line at the start of a file.

```java
#!/usr/bin/env zink
void main(string[.] args) {
    println("${args.len} arguments");
}
```

```output
0 arguments
```

```
$ chmod +x count
$ ./count a b c
3 arguments
```

- **The file doesn't need a `.zn` extension.** `zink FILE` runs any file that exists; only
  a word that is neither a file nor ends in `.zn` is taken as an unknown subcommand.
- **The first run compiles it**, and later runs use the cache (below), so from its second
  run on a script starts without the C compiler. The front end still runs, which for the
  830 lines of `examples/lisp.zn` takes under a tenth of a second.
- **To run a script at `-O2`**, pass `-O` through `env -S`:
  `#!/usr/bin/env -S zink run -O`.
- A script file can also start with settings such as `#max_memory_mb 64`, after the `#!`
  line ([Programs, packages and building](10-programs.md)).

## The `zink` command

| Command | What it does |
|---|---|
| `zink run [-O] FILE [args...]` | compile into the cache if needed, then run the program |
| `zink FILE [args...]` | the same as `zink run`, for `#!/usr/bin/env zink` |
| `zink build FILE -o OUT` | compile to an executable at `OUT`, with the C compiler at `-O2` |
| `zink check FILE...` | parse and type-check each file as its own program |
| `zink c FILE` | print the C generated for a program |
| `zink parse [--tree] FILE...` | report syntax errors; `--tree` prints the syntax tree |
| `zink lex FILE` | print the tokens of a file |
| `zink cache clean` | empty the cache |
| `zink --help` | print the list above |

### `zink run`

`zink run FILE` compiles the program, unless the cache already holds it, and then runs it in
place of the `zink` process (`exec`), so the program keeps the process ID, standard input,
output and error, signals and the exit status. Pipes, `$?` and Ctrl-C behave as if the
executable were run directly.

- **`zink`'s own flags go before the file**, and everything after the file goes to the
  program unchanged: in `zink run -O tool.zn -O -v`, the program gets `-O` and `-v`.
- **`-O` builds at `-O2`** instead of `-O0` (below, "Optimization").
- **Errors stop it before the program runs.** The compiler prints each error and a count,
  on standard error, and exits with 1.
- **Warnings don't stop it.** They're printed, and the program runs.

An error names the file, the line and the column, then shows the line with a caret under
what it's about, often with a `help:` line after it:

```java
void main() {
    i64 count = "three";
}
```

```error
example.zn:2:17: error: expected i64, found string
```

In full, `zink run` prints:

```
example.zn:2:17: error: expected i64, found string
        i64 count = "three";
                    ^^^^^^^
1 error in example.zn
```

The examples in this documentation show the first line of each message, and sometimes its
`help:` line.

### `zink build`

`zink build FILE -o OUT` writes an executable to `OUT`, built at `-O2`. It is a native
binary that needs nothing of Zinkite's to run: the runtime and the parts of the library the
program calls are compiled into it.

```
$ zink build greet.zn -o greet
$ ./greet ada
hello, ada
```

`-o` may come before or after the file. Without it, `zink build` prints its usage and exits
with 2.

### `zink check`

`zink check FILE...` parses and type-checks each file, and runs the reference checks of
[Memory and references](05-memory.md), without generating C. Each file is checked as a
program of its own. It ends with a summary on standard error, such as
`checked 2 files with no errors`, and exits with 1 when there was an error.

`zink check` doesn't generate C, so it accepts the few things the C backend doesn't support
yet; `zink run`, `zink build` and `zink c` report those as errors that say so
([What isn't there yet](15-not-yet.md)).

### `zink c`

`zink c FILE` prints the generated C on standard output. It is one C file, with the runtime
at its top, so even `hello.zn` gives a few thousand lines. Each function of the program is
near the end, under a mangled name such as `zm0_0_square`.

### `zink parse` and `zink lex`

These show the front end's work, for looking into the compiler or into a syntax error.

- **`zink parse FILE...`** reports syntax errors only, and prints
  `parsed 1 file with no errors` when there are none.
- **`zink parse --tree FILE`** also prints the syntax tree, one statement per line, as
  S-expressions.

For this `square.zn`:

```java
i64 square(i64 x) {
    return x * x;
}

void main() {
    i64 n = square(7);
    if (n > 40) {
        println("big: ${n}");
    }
}
```

```output
big: 49
```

the tree is:

```
$ zink parse --tree square.zn
i64 square(i64 x)
  (return (* x x))
void main()
  (local i64 n = (call square 7))
  (if (> n 40))
    (call println "big: ${n}")
parsed 1 file with no errors
```

and `zink lex FILE` prints each token with its line and column:

```
$ zink lex square.zn
1:1	Kw(I64)
1:5	Ident("square")
1:11	P(LParen)
...
```

### `zink cache clean`

`zink cache clean` empties the cache, the executables and what was read from C headers
alike, and says how many files it removed:

```
$ zink cache clean
removed 12 cached programs
```

## The cache

`zink run` keeps each executable it builds in the cache directory,
`$XDG_CACHE_HOME/zink`, or `~/.cache/zink` when `$XDG_CACHE_HOME` isn't set. Running the
same program again skips the C compiler, which is most of the time a first run takes:
about a quarter of a second for a small program
([DESIGN §14.4](../DESIGN.md#144-command-line-tool-decided)).

- **The key** is a hash of the generated C, the C compiler, its flags and the version of
  `zink`. So any change to the program that changes its C gives a new executable, as do
  `-O`, a different `$CC` and a new `zink`, and an executable built by an older `zink` is
  never run.
- **`zink run` still parses and checks the program on every run**, since the key is
  computed from the C. Only the C compiler is skipped.
- **Writes are atomic.** An executable is written under a temporary name and renamed into
  place, so two runs at once never see half of one.
- **The directory is private.** `zink` creates it with mode 0700, and refuses to run
  anything from it if it isn't owned by the user or others can write to it, since another
  user could otherwise plant a binary there.
- **Nothing is removed automatically.** `zink cache clean` empties it.
- What clang finds in C headers for `extern import` is cached in the same directory
  ([Calling C](12-calling-c.md)).

## The C compiler

`zink` uses the first of these that it finds:

1. `$CC`, when it is set and not empty;
2. `clang`;
3. `gcc`;
4. `cc`.

Without any, it stops with `no C compiler found; install gcc or clang, or set $CC`. clang
comes first because, measured on Zinkite programs, its code was as fast as gcc's or faster,
and it compiles faster at `-O0`
([DESIGN §14.2](../DESIGN.md#142-what-emitting-c-requires)).

- **The generated C has no undefined behavior**, so clang and gcc, at `-O0` and `-O2`, give
  the same results. Wrapping arithmetic goes through unsigned types, checked arithmetic
  through the compilers' overflow built-ins, and floating-point multiplies and adds aren't
  fused (`-ffp-contract=off`), so each rounds as written.
- **A C compiler error is a bug in `zink`**, never in the program: everything a program can
  get wrong is reported before the C is written. `zink` says so when it happens:
  `the C compiler failed on the generated code, which is a bug in zink`.
- **`extern import` reads headers with clang**: `$CC` when it names a clang, or `clang` on
  the path, whichever compiler builds the program ([Calling C](12-calling-c.md)).

## Optimization: `-O0` and `-O2`

`zink run` builds at `-O0`, which compiles fastest; `zink run -O` and `zink build` build at
`-O2`, which runs fastest.

- **The results are the same** at both levels; only the speed differs.
- **`-O0` code is slower in hot loops**, often several times slower. A script that does
  real work in a loop is worth running with `zink run -O`, or building once with
  `zink build`.
- **At `-O2` on x86-64**, `zink` also keeps jumps off 32-byte boundaries, which on some
  Intel CPUs keeps a hot loop in the decoded-instruction cache
  ([DESIGN §12.5](../DESIGN.md#125-backends-and-executables-decided)).
- **`zink build` always builds at `-O2`.** There is no debug build; `zink run` is the quick
  one.

## A tour of a larger program

This program reads lines of `name score`, here from a string in the program, and prints
each student's average and letter grade. It uses most of what the next chapters describe.

```java
#!/usr/bin/env zink
// grades: each student's average and letter grade, from "name score" lines.

class Student {
    string name;
    i64 total;
    i64 count;

    f64 average() {
        return (f64) total / (f64) count;
    }
}

string letter(f64 average) {
    return switch ((i64) average / 10) {
        case 10, 9 => "A";
        case 8 => "B";
        case 7 => "C";
        case 6 => "D";
        default => "F";
    };
}

// The student with this name, added at the end if there isn't one yet.
Student& find(Student[..>]& students, string name) {
    for (Student& s : students) {
        if (s.name == name) {
            return s;
        }
    }
    return students.push(new Student(name, 0, 0));
}

void main() {
    string scores = "ada 91\nbo 78\nada 85\ncy 64\nbo x\ncy 70\nbo 80\n";
    Student[..>] students = [];
    i64 lineNo = 0;
    for (string line : scores.lines()) {
        lineNo++;
        (string name, string score) = line.splitFirst(" ");
        i64 n @bad = i64.parse(score);
        if (bad) {
            println("line ${lineNo}: `${score}` isn't a number");
            continue;
        }
        Student& s = find(students, name);
        s.total += n;
        s.count++;
    }
    for (Student& s : students) {
        f64 avg = s.average();
        println("${s.name}: ${avg.fixed(1)} (${letter(avg)})");
    }
}
```

```output
line 5: `x` isn't a number
ada: 88.0 (B)
bo: 79.0 (C)
cy: 67.0 (D)
```

From the top:

- **`class Student`** has three fields and a method. It has no constructor, so it gets a
  default one that takes each field in order: `new Student(name, 0, 0)`. `average` reads
  the fields by their bare names, and doesn't change them, so it needs no `mut`
  ([Classes, interfaces and generics](06-classes.md)).
- **`(f64) total`** converts an integer to a float. Integers don't turn into floats on
  their own when information could be lost ([The basics](02-basics.md#casts)).
- **`switch`** is an expression here, with `=>` arms and no fall-through, and `default`
  makes it cover every value ([The basics](02-basics.md#switch)).
- **`Student& find(Student[..>]& students, ...)`** takes the array by reference, since it
  adds to it, and returns a reference into it: either an element it found, or the one
  `push` added. `Student[..>]` is an array that grows only at its end
  ([Arrays](04-arrays.md)); `&` marks a reference
  ([Functions and lambdas](08-functions.md), [Memory and references](05-memory.md)).
- **`string`** is a view of bytes kept elsewhere, here in the literal. `name` and `score`
  are views into it, so no text is copied, and `Student.name` keeps one
  ([Text](03-text.md)).
- **`scores.lines()`** gives the lines one at a time, without building an array of them
  ([Maps, sets, pools and iterators](09-collections.md)).
- **`(string name, string score) = line.splitFirst(" ");`** takes the two results of one
  call ([Functions and lambdas](08-functions.md#multiple-results)).
- **`i64 n @bad = i64.parse(score);`** reads the status of the call. A failed parse gives
  0 with a nonzero status, `INVALID`, instead of throwing; without `@bad`, the bad line
  would stop the program ([Errors and statuses](07-errors.md)).
- **`if (bad)`** works because unsigned integers are conditions: 0 is false
  ([The basics](02-basics.md#bool-and-conditions)).
- **`Student& s = find(...)`** binds a reference, so `s.total += n` changes the element in
  the array, not a copy. The compiler checks that no reference outlives what it points
  into, without annotations ([Memory and references](05-memory.md)).
- **`${avg.fixed(1)}`** formats a float with one digit after the point
  ([Text](03-text.md)).

## Where to go next

- [The basics](02-basics.md), [Text](03-text.md) and [Arrays](04-arrays.md) are enough to
  write small programs.
- [Memory and references](05-memory.md) is what makes Zinkite different from the languages
  it looks like: there is no heap, and references are checked at compile time. Read it
  before writing a program that keeps references.
- [Classes, interfaces and generics](06-classes.md), [Errors and statuses](07-errors.md)
  and [Functions and lambdas](08-functions.md) cover the rest of the language.
- [Maps, sets, pools and iterators](09-collections.md), [Threads](11-threads.md) and
  [Calling C](12-calling-c.md) cover the built-in collections, workers and C libraries.
- [Programs, packages and building](10-programs.md) covers programs of several files.
- [The library: files, processes, time and the command line](13-library-system.md) and
  [The library: data](14-library-data.md) cover the standard library.
- [What isn't there yet](15-not-yet.md) lists what is designed and not written.

The programs in [`examples/`](../examples) are complete tools, from a small `sort(1)` to a
Lisp interpreter; [`examples/NOTES.md`](../examples/NOTES.md) says what writing them
showed.
