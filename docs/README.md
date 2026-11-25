# The Zinkite documentation

Zinkite is a small, statically typed language for Linux command-line tools and embedded
scripts. It has no heap, no garbage collector and no reference counting, and it checks
memory safety at compile time, without lifetime annotations. The compiler, `zink`, turns a
program into C and builds it with clang or gcc.

This documentation describes the language as the compiler implements it today, a feature
at a time, with examples that are compiled and run by the compiler's tests. The reasons
behind each decision, and the alternatives that were rejected, are in the design documents
it links to.

```java
void main() {
    string who = "world";
    println("hello, ${who}");
}
```

```output
hello, world
```

## Chapters

| | Chapter | What it covers |
|---|---|---|
| 1 | [Getting started](01-getting-started.md) | installing `zink`, running and building programs, scripts with `#!`, the cache |
| 2 | [The basics](02-basics.md) | values and types, numbers and their arithmetic, casts, variables, operators, statements and control flow, `main` |
| 3 | [Text](03-text.md) | `string` views and `u8` arrays, interpolation, printing and formatting, text methods, UTF-8 |
| 4 | [Arrays](04-arrays.md) | the slot grammar, every kind of array, operators, indexing, slices and writable views, sorting and the other methods |
| 5 | [Memory and references](05-memory.md) | data stacks, copies and references, roots and lifetimes, removals, variable-size records, links, roots with several arrays |
| 6 | [Classes, interfaces and generics](06-classes.md) | classes, constructors, visibility, `mut`, statics, interfaces, enums, generics and bounds, `packed`, `defer` |
| 7 | [Errors and statuses](07-errors.md) | the status code: `#`, `@`, `@@`, `@@@`, repeat loops, failures nothing reads, named codes |
| 8 | [Functions and lambdas](08-functions.md) | functions, overloading, lambdas and function types, multiple results, recursion |
| 9 | [Maps, sets, pools and iterators](09-collections.md) | `hashmap`, `hashset`, `pool` and `Handle`, iterators and for-each |
| 10 | [Programs, packages and building](10-programs.md) | script files and packages, imports, settings, `#if`, static initialization, the `zink` command, `simd` |
| 11 | [Threads](11-threads.md) | workers that share no memory, `queue<T>`, what a worker takes with it |
| 12 | [Calling C](12-calling-c.md) | `extern import`, `extern link`, explicit declarations, handles |
| 13 | [The library: files, processes, time and the command line](13-library-system.md) | `zink.lang`, `zink.io`, `zink.os`, `zink.time`, `zink.cli` |
| 14 | [The library: data](14-library-data.md) | `zink.random`, `zink.hash`, `zink.encoding`, `zink.csv`, `zink.data`, `zink.json` |
| 15 | [What isn't there yet](15-not-yet.md) | features designed but not implemented, open decisions, known limits |

Chapters 2 to 4 are enough to write small programs. Chapter 5 is the one that makes
Zinkite different from the languages it looks like, and the one to read before writing a
program that keeps references.

## The other documents

| Document | What it holds |
|---|---|
| [`DESIGN.md`](../DESIGN.md) | every decision, its status, its reasons, and what was rejected |
| [`GRAMMAR.md`](../GRAMMAR.md) | the grammar, and how the parser resolves what's ambiguous |
| [`REFERENCES.md`](../REFERENCES.md) | the reference rules the compiler checks, precisely |
| [`STDLIB.md`](../STDLIB.md) | the standard library's design, including what isn't written yet |
| [`compiler/README.md`](../compiler/README.md) | how the compiler works and what it covers |
| [`examples/`](../examples) | complete programs, and [`NOTES.md`](../examples/NOTES.md) on what writing them showed |

## Examples

Zinkite code is shown in ` ```java ` blocks, which most viewers highlight well enough. Every
complete program, one that declares `main`, is compiled by the compiler's tests
(`compiler/tests/docs.rs`), so the examples here can't drift from the language:

- a program must compile with no errors or warnings;
- an ` ```output ` block after it holds exactly what it prints;
- a ` ```stderr ` block holds lines its standard error must contain, as when it stops;
- an ` ```error ` block holds lines the compiler must print for it, which then must not
  compile. A ` ```warning ` block does the same for a program that compiles with warnings.

Messages name the program `example.zn`. Fragments, without `main`, aren't checked on their
own, and are always parts of a checked program nearby or of the library's real code.

To check the examples after editing a chapter:

```
cd compiler
cargo test --release --test docs
ZINK_DOCS=03-text.md cargo test --release --test docs    # one chapter
```
