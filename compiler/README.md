# The Zinkite compiler

The `zink` command, written in Rust (DESIGN §14.1): a lexer and a parser for
`GRAMMAR.md` v2, the type checker, the lifetime checks of `REFERENCES.md`, and a
C backend whose first milestone runs programs that use the core language.

## Building and testing

```
cargo build
cargo test
cargo clippy --all-targets
```

The parser tests parse every program in `../examples` and the complete example
in `GRAMMAR.md` §7, check each disambiguation rule of GRAMMAR §6, and parse
every prefix of the examples to make sure broken input gives errors rather than
a crash.

The checker tests type-check the same programs, which must give no errors, and
then check each rule with a program that breaks it and the message it gives. A
last test deletes or repeats each line of each example and checks the result,
to make sure the checker never crashes.

## Using it

```
zink run [-O] FILE [args]   compile into the cache if needed, then run the program
zink FILE [args]            the same, for `#!/usr/bin/env zink`
zink build FILE -o OUT      compile to an executable with the system C compiler at -O2
zink c FILE                 print the generated C
zink cache clean            empty the cache (`~/.cache/zink`)
zink check FILE...          parse and type-check each file as its own program
zink parse FILE...          report syntax errors, with the line and a caret
zink parse --tree FILE      print the syntax tree, one statement per line
zink lex FILE               print the tokens
```

## What the checker covers

Done:
- names: locals, fields, statics, classes, imports, type parameters, and a
  local hiding a class;
- types: literals typed by where they are used, the implicit conversions, the
  mixed-integer rule, casts, conditions, `null` and narrowing;
- calls: overloading and its ranking, generic inference and bounds, lambdas,
  constructors (including default ones and `this.new`), and free functions
  called as methods;
- arrays: which methods and operators each slot type allows, slices and
  writable views;
- statuses: `#` and `@`, repeat loops, and a warning when a status that can be
  nonzero is dropped;
- statements: `switch` (exhaustive as an expression), `yield`, labels, `defer`,
  tuples, for-each over every kind of collection and iterator, and missing
  `return`s;
- declarations: interfaces and `implements ordered`, overloads that differ only
  in `&`, hashable keys, and the field rules for classes and roots;
- definite assignment (DESIGN §6.2): every field assigned on every path through
  a constructor, judged by the worst case, with failing exits, `this.new`, the
  fields that have no zero value, and no use of `this` before then; and no local
  read before every path has assigned it.

- `mut` and read-only checks: plain parameters are read-only copies, a method
  that isn't `mut` can't change its object or call one that is, a writable view
  needs an array the code may change, and a lambda's captures are read-only and
  never assigned after they're initialized. A reference bound to something the
  code can't change can't be written through.

- roots and lifetimes (`REFERENCES.md`), after type checking: rule S (no
  reference outlives what it points into), with a summary inferred for every
  function; rule R (no removal from an array while a reference into it is still
  used, including removals by called functions, for-each loops and `defer`);
  references into removable arrays kept only in locals and parameters, also
  when the array is a field of the same object (each such field is storage of
  its own, like a root's part); no
  recursive call while a growable local is in scope; and no stored lambda that
  captures. A root can't be copied, passed or returned by value, or be a
  temporary (DESIGN §6.6).

Not yet:
- rules V (variable-size values) and L (links) of `REFERENCES.md`, and rule G's
  check that nothing grows an array while a value is built in it;
- the lint for unused array capabilities (DESIGN §4.3).

The standard library isn't written yet. The checker models `zink.lang`
(output, `status`, `min`, `max` and the rest), `math`, and the methods on
numbers, strings, arrays, `hashmap`, `hashset` and `pool`. Names from the other
`zink.*` packages, such as `File` and `Args`, aren't checked, and a file that
imports one gets a warning saying so. For the lifetime rules, a call into one of
those packages is taken to return something rooted at everything it was given,
and to remove from nothing.

## The C backend

`zink run` and `zink build` generate one C file for the program, with the runtime
(`runtime/zink_rt.h`) at its top, and compile it with the system C compiler:
`$CC`, `clang`, `gcc` or `cc`, in that order (DESIGN §12.5, §14.2). `zink run` builds at `-O0`
and caches the executable by a hash of the C; `-O` builds at `-O2`. A built-in
TinyCC for `zink run` is a maybe-later (DESIGN §17).

It covers script files, classes, enums, constructors and methods, statuses,
every number type including `i128` and `u128` (as `__int128`),
references, strings and interpolation, every array kind, slices (except of cursor,
chunked and reversed arrays on data stacks, which are undecided), every statement
form, casts and the integer rules, lambdas and function values, hash maps and
sets, and for-each over arrays, maps, sets and text iterators. Bugs stop the
program with the file and line, and exit status 1. `examples/linebuf.zn` and
`examples/wordfreq.zn` run.

Lambdas (DESIGN §9.3): one passed to a built-in, such as a comparator, key or
predicate, is inlined where the built-in uses it, its parameters pointing at the
elements, so it costs what a handwritten loop would and may capture anything in
scope. One used as a value is a C function reached through a pointer, as a stored
`func` can't capture. The sort family (`sort`, `stableSort`, `sortBy`,
`sortByDescending`, `radixSortBy`, `partialSort` and the others) is a stable merge
sort; `partialSort` keeps a heap of the smallest, O(n log k). Also `isSorted`,
`findIndex`, `any`, `all`, `count`, `retain`, `dedup` and `fold`.

`hashmap` and `hashset` (DESIGN §4.14) follow Python's compact dict on one data
stack: entries appended in insertion order, never moved by an insert; an index of
1-, 2-, 4- or 8-byte slots, as narrow as the capacity allows, at half the stack's
reservation, rebuilt when the entries outgrow it; removal marks an entry dead and
compacts once more than half are. String keys keep their hash. Hashing mixes with a
128-bit multiply (wyhash's step) and a random seed per run. Keys can be integers,
`bool`, enums and strings; class keys aren't supported yet.

Not yet: generics, interfaces, a capturing lambda passed to a function of the
program's own (which needs that function specialized per lambda), `filter`, `map`
and the other methods that build a new array, iterators in variables, comparing
class values, and the library packages that aren't written yet (`zink.io`,
`zink.cli` and the others). Of `stdin`, only `readAll` and `nextLine` work so far.
Using any of the rest is a clear error, not wrong code.

Arrays on data stacks, by layout:
- changing only at the end (`T[..>]`, `T[..~]`, frozen ones): a header `{p, n}`
  pointing at the elements, rather than the inline `[metadata][elements]` of
  DESIGN §11;
- changing at the start too (`T[~.~]`, `T[>..]`, `T[<.<]`): contiguous elements
  starting in the middle of their reservation, so either end can grow;
- with a cursor (`T[.~.]`, `T[~]`): the layout of DESIGN §4.3 and §4.5, as
  prototyped in `bench/gap_buffer.c`. Elements live in a data array and never
  move; removals leave holes that the next insert reuses; the order is an index
  of 32-bit slots, kept as two deques, before and after the cursor. Two data
  stacks: the data, and the index;
- with a chunked index (`T[.~.$]`, `T[~$]`): the same data array, with the index
  split into chunks of 2^L entries, each a circular buffer: a two-level tiered
  vector, as in `bench/gap_buffer.c`. Edits anywhere cost O(√n), reads take three
  loads, and the cursor is just a position. The chunk size doubles when the length
  passes 4 B², which keeps it between √n/2 and √n.

Each declaration of a growable local gets data stacks of its own.

Capped arrays (`T[..~16]`) sit inline in their parent, with counters as narrow as
the cap allows (DESIGN §11):
- `{n, e[k]}` when only the end changes;
- with a start slot (`T[~.~8]`): a ring buffer `{h, n, e[k]}`. One that changes
  only at the start (`T[>..8]`, `T[~..8]`) sits at the top of the buffer, so it
  never wraps;
- with a cursor (`T[~8]`, `T[.~.8]`): a gap buffer `{gs, an, cur, e[k]}`, the
  elements before the gap at the start of `e` and those after it at the end. The
  cursor is kept apart: seeks are free, and the gap moves to the cursor only for an
  edit there, as in Emacs.

Rule R covers capped arrays too (REFERENCES §5.4): a removal ends references into
one. Gap buffers and rings that change at both ends can move elements, so an insert
into one ends references and slices into it, and a slice, sort or comparison of one
ends references to its elements.

A slice of a ring or gap buffer whose elements are split first joins all of them:
the ring rotates so its first element is at `e[0]`, or the gap moves to whichever
end moves fewer. Later slices then move nothing until the next edit, so slices taken
since the last edit agree (DESIGN §4.10). A slice of a temporary is copied to the
scratch stack, so it lasts to the end of its statement.

A literal longer than the cap doesn't compile, and text or a slice that's longer
stops the program, as a cast that doesn't fit does (DESIGN §9.1).

## Layout

| File | Contents |
|---|---|
| `src/source.rs` | source files and byte spans |
| `src/diag.rs` | diagnostics and how they are printed |
| `src/token.rs` | tokens, keywords and operators (GRAMMAR §1) |
| `src/lexer.rs` | text to tokens, including string interpolation and `#` directives |
| `src/ast.rs` | the syntax tree |
| `src/parser.rs` | tokens to the syntax tree (GRAMMAR §2–§6) |
| `src/sexpr.rs` | the tree as compact S-expressions, for tests and `--tree` |
| `src/types.rs` | the checker's types, array kinds and generic bounds |
| `src/program.rs` | every declaration in a program, and resolving written types |
| `src/builtins.rs` | signatures of the built-in functions and methods |
| `src/check.rs` | the type checker |
| `src/flow.rs` | the state definite assignment tracks: what is assigned on every path, and on some |
| `src/lifetimes.rs` | roots and lifetimes: rules S and R, function summaries, recursion |
| `src/codegen.rs` | the C backend |
| `src/driver.rs` | compiling the C, the cache, and running the result |
| `runtime/zink_rt.h` | the runtime: data stacks, output, failures, checked arithmetic, text |
| `src/main.rs` | the `zink` command |
| `tests/parse.rs` | parser tests |
| `tests/check.rs` | type checker tests |
| `tests/lifetimes.rs` | lifetime tests, from `REFERENCES.md` §10 |
| `tests/run.rs` | programs compiled to C, built and run |
