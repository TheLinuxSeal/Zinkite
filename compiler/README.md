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
prefixes of the examples, cut every few bytes, to make sure broken input gives errors
rather than a crash.

The checker tests type-check the same programs, which must give no errors, and
then check each rule with a program that breaks it and the message it gives. A
last test deletes or repeats each line of each example, or 200 lines spread through
a long one, and checks the result, to make sure the checker never crashes.

The documentation's tests (`tests/docs.rs`) compile every complete program in
`../docs`, run those followed by an `output` block and compare what they print, and
check that those followed by an `error` block are rejected with its message
(`docs/README.md`, "Examples"). `ZINK_DOCS=03-text.md` checks one chapter.

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
- statuses: `#`, `@`, `@@` and `@@@`, and repeat loops;
- statements: `switch` (exhaustive as an expression), `yield`, labels, `defer`,
  tuples, for-each over every kind of collection and iterator, and missing
  `return`s;
- declarations: interfaces and `implements ordered`, overloads that differ only
  in `&`, hashable keys, and the field rules for classes, roots and `packed`
  classes, which are packed C structs whose fields no reference may point at
  (DESIGN §6.1);
- visibility (DESIGN §6.1): private members, package-private ones (nothing
  written), and a default constructor as visible as the fields it sets;
- definite assignment (DESIGN §6.2): every field assigned on every path through
  a constructor, judged by the worst case, with failing exits, `this.new`, the
  fields that have no zero value, and no use of `this` before then; and no local
  read before every path has assigned it.

- `mut` and read-only checks: plain parameters are read-only copies, a method
  that isn't `mut` can't change its object or call one that is, a writable view
  needs an array the code may change, and a lambda's captures are read-only and
  never assigned after they're initialized. A reference bound to something the
  code can't change can't be written through. A `final` field is set only by its
  initializer or else its class's constructors, and a `static final` only by its
  initializer (DESIGN §6.1).

- constants (DESIGN §6.1): an integer, `bool` or `string` `static final` set by
  a constant expression is worked out exactly at compile time, in
  `program.rs`, where a cap may need it while types are resolved; it can give
  an array's capacity and be a case label, and the C has an integer or `bool`
  one's value, not a thread-local copy.

- roots and lifetimes (`REFERENCES.md`), after type checking: rule S (no
  reference outlives what it points into), with a summary inferred for every
  function; rule R (no removal from an array while a reference into it is still
  used, including removals by called functions, for-each loops and `defer`);
  references into removable arrays kept only in locals and parameters, also
  when the array is a field of the same object (each such field is storage of
  its own, like a root's part); no
  recursive call while a growable local is in scope, or while `filter`, `map` or
  `sortedIndexes` is building its array; and no stored lambda that captures.
  What a lambda returns is part of what a built-in like `map` or `fold`
  returns, so text it builds for the statement can't be kept past it, and a
  lambda passed anywhere but straight to a built-in can't return such text at
  all. A root can't be copied, passed or returned by value, or be a temporary
  (DESIGN §6.6).

- threads (DESIGN §12.9): what a spawned lambda uses from outside it, and what a
  `queue` holds, is flat, checked with the reason when it isn't; a loop's counter may
  change after a spawn captures it, since the worker has a copy. After the lifetimes
  pass, the code a worker may run, its lambda and everything that calls, through
  interfaces and, conservatively, function values, may reach only the statics a new
  thread gets copies of, and of those with views only ones of string literals.

- rule V (variable-size values, `REFERENCES.md` §6): a variable-size field isn't
  assigned once its record is built (V1), though a fixed-size field of it can be;
  assigning a whole variable-size variable is a removal under rule R (V2); an array
  of variable-size values is only added to at its end, cleared and looped over, by
  reference, never indexed, sliced, popped, reordered or given a cursor (V3). V4 is
  met by the code generator rather than refused: a record whose building calls
  code that could grow its array is built aside and moved to the array's end. A
  constructor of a variable-size class assigns its variable-size fields in order,
  each by a statement of its own (V5). A variable-size local, like a growable one,
  can't be in scope during a recursive call (§8.4).

- rule L (links, `REFERENCES.md` §7): a self-relative or pool link points into its own
  array, checked with the inferred roots through calls and constructors (L1); a value
  with self-relative links is built in its array by `push(new ...)` and never copied
  (L2), after its constructor's arguments are evaluated; and an offset that doesn't
  fit its width stops the program (L4). A link field is its 1-, 2- or 4-byte offset,
  and an array of links (`T&<i32>[..~k]`) a capped array of them, each from its own
  slot, which takes `+>`, `[i]`, `for`, `.len` and `clear` so far. A link written
  anywhere else is an error.

Not yet:
- rule G's check that nothing grows an array while text is built in it;
- the lint for unused array capabilities (DESIGN §4.3).

The checker models `zink.lang` (output, `status`, `min`, `max` and the rest),
`math`, and the methods on numbers, strings, arrays, `hashmap`, `hashset` and
`pool`. `zink.io` and `zink.cli` are written in Zinkite (below) and checked like
the program's own files, as are the other `zink.*` packages written so far. Names
from the packages that aren't written yet, such as `zink.toml`, aren't checked,
and a file that imports one gets a warning saying so. For the lifetime rules, a call into one of those packages is
taken to return something rooted at everything it was given, and to remove from
nothing.

## The C backend

`zink run` and `zink build` generate one C file for the program, with the runtime
(`runtime/zink_rt.h`) at its top, and compile it with the system C compiler:
`$CC`, `clang`, `gcc` or `cc`, in that order (DESIGN §12.5, §14.2). `zink run` builds at `-O0`
and caches the executable by a hash of the C; `-O` builds at `-O2`. A built-in
TinyCC for `zink run` is a maybe-later (DESIGN §17).

It covers script files, classes, enums, constructors and methods, statuses,
every number type including `i128` and `u128` (as `__int128`),
references, interfaces, strings and interpolation, every array kind, slices (except of cursor,
chunked and reversed arrays on data stacks, which are undecided), every statement
form, casts and the integer rules, generics, lambdas and function values, hash
maps, sets and pools, calling C, and for-each over arrays, maps, sets, pools and
iterators. Every failure is a status (DESIGN §7.6): `@` reads any in its expression,
calls, indexes, arithmetic and casts alike, by jumping to a local label where the
status is read; one that nothing reads stops the program at its line, naming what
failed and its trail through the program's own functions, with exit status 1. That
check is one predictable branch after a call that can fail. `@@` and `@@@` catch such a
failure deep inside too: a `setjmp` at the `@@`, which the stop jumps back to instead,
through a catch record for each `defer` on the way; the trail is a fixed buffer of 32
entries that each failing `return` writes to. All ten
examples run: `cachesim`, `csvsort`, `dirwalk`, `hashtree`, `jsonfmt`, `linebuf`,
`lisp`, `sortlines`, `stats` and `wordfreq`. So does `quarry/quarry.zn`, a voxel
sandbox drawn with OpenGL through SDL2, which the tests build where their headers are
installed.

Threads (DESIGN §12.9, `codegen/threads.rs`): a program that imports zink.thread is
compiled with `ZR_THREADS`. The runtime's per-thread state, the data stacks' table, the
scratch stack, the trail, `@@` records and the open Files' table, and the program's
statics, are `__thread` then, so each worker is the program over again; output takes
stdout's lock, a statement's line at a time, after its parts are worked out, and the
environment a mutex. A spawned lambda is a C function, `ztN`, given a packet the
spawning thread fills with its captures' bytes and the statics' (an array's elements
too), which it unpacks into its own; workers are detached pthreads with a record each
that `join` waits on. A queue is a mutex, a condition variable on the monotonic clock,
and a list of nodes, each one value's bytes. A worker's data stacks are unmapped, and
its Files written out and closed, when it ends: 300 workers in turn that each fill 8 MB
peak at 10 MB. ThreadSanitizer finds nothing in the tests' programs, built with it, which
send output through `fwrite`, as it doesn't model `flockfile`. A program that doesn't
import zink.thread compiles to the same C as before. `hashtree` hashes `/usr/include`,
152 MB in 12,103 files, in 0.69 s with one worker and 0.10 s with 18.

Variable-size records (DESIGN §3.2, §11): a class with a `T[.*]` or `T[.]` field, or
a field that is such a class, is one run of bytes. Its fixed-size fields come first,
as a packed C struct sorted by size, then each variable-size field in declaration
order: an array as its count (a varint for `T[.*]`, 8 bytes for `T[.]`) and its
elements, a record as its bytes (`codegen/records.rs`). A variable of the class is a
pointer to its record on a data stack of its own, and a parameter of it is passed by
pointer. `new C(...)` writes the record where it goes, field after field, nested
array literals included, so `book +> new Order(1, "alice", [new Item("SKU-7", 1,
500)])` writes the order once, at the end of `book`. An array of variable-size values
is `zr_rvec`, which keeps its byte length beside its count; a for-each walks it with
each type's size function. A million items with a short SKU take 16 MB inline,
against 35 MB as `string` views into one buffer, and 20 passes over them take
0.10 s against 0.07 s, since finding the next record decodes its lengths.

A constructor of a variable-size class is a C function that writes the record at a
destination and gives its length: fixed-size fields through the packed struct,
variable-size ones at the end of what's written so far, which V5 makes possible; a
failing one writes the zero record. A function returning a record takes the same
hidden destination an array result does (DESIGN §3.3): the local it returns starts
there, `return new R(...)` builds there, and `return g(x);` passes it on, so
`Item m = make("m1");` writes the item once, in `m`. Not yet: variable-size values
in hash maps, pools and tuples.

`pool<T>` (DESIGN §3.5) is one grow-only array of slots on a data stack, each
`{generation, next free, value}`, with the free slots in a list through `next`
(`zr_pool` in the runtime, a slot struct per element type). A slot's generation is
odd while it holds a value and even once freed, so a stale handle fails with
`MISSING`; the value stays, so a stale reference reads a value of the type. `add`
takes the last slot freed before growing; a for-each visits the live slots in slot
order. A `Handle<T>` is the slot's index and generation, 8 bytes. A handle whose slot
the pool never had, from another pool or a failed call's zero value, stops the
program as a bug. `cachesim` replays 200,000 keys through its LRU cache in 10 ms, and
gives the same hits as a Python `OrderedDict` at every capacity tried.

A function's array result is built where the caller keeps it (DESIGN §3.3). A
function returning a `T[.]`, `T[..>]` or `T[..~]` takes a hidden first argument: the
storage to build it in, or `NULL`. The local it returns by name starts there instead
of on a data stack of its own, so a declaration such as `u8[.] text = f(x);` is
written once. `return g(x);` passes the storage on, and any other `return` copies into
it. Two cases still copy, once:
- an assignment to an array that already exists, `x = rev(x)`, since the arguments
  may be reading it: the call builds on its own stack and the result is copied in;
- a temporary, as in `path.relative(path.absolute(a), path.absolute(b))`: it is
  built on the callee's stack and copied to the scratch stack, where it lasts to the
  end of its statement.

Building a 200 MB array this way takes 0.26 s and 197 MB, against 0.39 s and 392 MB
when it is copied.

Lambdas (DESIGN §9.3): one passed to a built-in, such as a comparator, key or
predicate, is inlined where the built-in uses it, its parameters pointing at the
elements, so it costs what a handwritten loop would and may capture anything in
scope. One that captures, passed to a function of the program's own, gets a copy
of that function, generated where it calls the lambda with the lambda inlined
there, and passed pointers to the variables the lambda reads and to `this` when
it was written in a method. A copy that passes the lambda on, to a built-in, to
another function or to itself, passes it on inlined too, and a new lambda around
it, as in `countIn(xs, (i64 x) => !keep(x))`, gets a copy that inlines both. A
function that wraps it in a new lambda for every call to itself is an error, as
it would need copies without end. One used as a value is a C function reached
through a pointer, as a stored `func` can't capture. The sort family (`sort`,
`stableSort`, `sortBy`, `sortByDescending`, `radixSortBy`, `partialSort` and the
others) is a stable merge sort; `partialSort` keeps a heap of the smallest,
O(n log k). Also `isSorted`, `findIndex`, `any`, `all`, `count`, `retain`,
`dedup` and `fold`.

`filter`, `map` and `sortedIndexes` build their array where the result goes, as a
function returning one does (DESIGN §3.3): in a new variable's storage, or on a
data stack of their own, copied to the scratch stack when the result is a
temporary. `sortedIndexes` merge-sorts the indexes, so it works on read-only
slices. The elements, and what `map` returns, are values a C assignment copies:
not arrays, variable-size records, values with links, or classes with a growable
field.

`hashmap` and `hashset` (DESIGN §4.14) follow Python's compact dict on one data
stack: entries appended in insertion order, never moved by an insert; an index of
1-, 2-, 4- or 8-byte slots, as narrow as the capacity allows, at half the stack's
reservation, rebuilt when the entries outgrow it; removal marks an entry dead and
compacts once more than half are. String keys keep their hash. Hashing mixes with a
128-bit multiply (wyhash's step) and a random seed per run. Keys can be integers,
`bool`, enums and strings; class keys aren't supported yet.

Generics are monomorphized (DESIGN §10): each generic function, and each method
and constructor of a generic class, is generated once per set of type arguments it's
used with, on demand, with every type the checker recorded substituted; each generic
class gets a struct per instantiation, named before its fields so a type like
`Node<K>` can refer to itself. In an instantiation, a type parameter bounded by an
interface is a known class, so its methods are called directly. Instantiation that
grows its type arguments without end is rejected.

Interfaces (DESIGN §6.3, §12.4): a `Shape&` is two pointers, to the value and to
the method table of its class for `Shape`. There is one table per class and
interface (and per instantiation of either), in static data, made the first time
the pair is needed. Each entry is a thunk that takes the value as `void *` and
calls the class's method directly, adding a status of 0 when the interface's method
can fail and this class's can't. `instanceof` compares the table pointer. A call
through an interface costs an indirect call: 100 million `area()` calls over a
`Shape&[..>]` take 0.22 s, against 0.11 s for direct calls over the two concrete
arrays. A for-each over an iterator is a repeat loop on a hidden local (DESIGN
§9.4), on any class with `mut T next()`, a class with `iterator()`, or an
`Iterator<T>&`.

Calling C (DESIGN §12.6): `extern import stdio.h;` runs clang over the header
(`src/cheader.rs`), which lists its functions, typedefs, enum constants and macros.
Functions whose signatures have one clear meaning are called directly: numbers,
enums (a typedef of an unnamed one, as SDL writes its types, included),
`const char *` as text copied with a NUL onto the scratch stack, and pointers to C
structs as handles. The generated C includes the header, so the C compiler checks
every call. An explicit `extern` declaration gets a prototype bound to the C symbol
by an asm label, so it can't clash with the header's own. `extern link m;` reaches
the driver through a comment at the top of the C. What clang finds is cached in the
cache directory, keyed by the headers' names, the clang binary and the version of the
reader, and kept while every file clang read is unchanged; a cached read checks those files' sizes and times
and starts no process.

The standard library (DESIGN §12.7) is Zinkite in `lib/zink/`, built into the
`zink` binary (`src/stdlib.rs`) and added to a program when one of its files
imports a package, as further files with spans of their own. `zink.lang` and
`zink.io` are in every program, since `stdin`, `stdout` and `stderr` are `File`s.
Only the library functions a program calls are generated, the way instantiations of
generics are, so a program that prints "hi" has none. Its C layer is the
runtime's `zr_` functions, declared as `extern`s that take Zinkite's
representations as they are. Written so far:
- `zink.io`: `File` (open, create, append, createNew, update; read, readLine,
  nextLine, readAll; write, print, println; seek, position, size, isTerminal,
  sync, close; readAll, writeAll and appendAll of whole files, writeAll by a
  temporary file and a rename; each `readAll` both returning the text and appending
  it to a buffer), `Dir` and `DirEntry`, `Stat`, `fs` (exists,
  stat, lstat, remove, removeAll, rename, move, copy, mkdir, mkdirAll, chmod,
  symlink, readLink, tempFile, tempDir, walk) and `path` (join, base, dir, ext, stem,
  isAbsolute, normalize, absolute, relative, resolve, cwd, chdir). A File's writes
  are buffered, 64 KB at a time, and written out at exit however the program ends;
  a million `println` calls to a file take 0.05 s and 184 system calls;
- `zink.cli`: `Args`, with the `--help` it generates, `ArgIterator` for `getAll`
  and `restValues`, and `ansi`'s `Colors`;
- `zink.lang`: `env` (get, has, set, unset, all) besides what's built in;
- `zink.os`: `process` (run, exec, pid, name, which), `Exit`, `Command` (arguments,
  working directory, environment changes, input, timeout; run, output, outputBoth,
  spawn), `Child` (wait, kill, killNow) and `term.size`. A program is started with
  `posix_spawnp`, SIGPIPE back to its default, and one `poll` loop feeds its stdin
  and reads its stdout and stderr;
- `zink.time`: `time` (now, monotonic, sleep), `Duration`, `Instant`, `Timestamp`
  and `DateTime` (RFC 3339 both ways, weekdays), with Howard Hinnant's calendar
  algorithms; they agree with Python's `datetime` over 5,000 random instants;
- `zink.random`: `Rng`, xoshiro256** seeded through SplitMix64, with Lemire's
  unbiased `range` for every integer type, and the kernel's bytes for secrets;
- `zink.thread`: `thread` (`cores`; `spawn` is the compiler's) and `Worker` (`join`),
  besides the built-in `queue<T>`;
- `zink.hash`: FNV-1a, CRC-32, all of SHA-2 and SHA-3, and SHAKE, one-shot and
  incremental, in the runtime's C, with one state for every SHA algorithm and an
  unrolled Keccak; they agree with Python's `zlib` and `hashlib` at every padding and
  block boundary. `xxh3` waits for a reference to test against;
- `zink.encoding`: `bytes.hex()`, `hex.decode` and base64 with both alphabets;
- `zink.csv`: `CsvReader`, with fields as views into the input where they can be,
  and `CsvWriter`; 2,000 random rows round-trip through Python's `csv`;
- `zink.data`: the tree every parser builds, `Node`s in document order with each
  subtree's size, so navigating is index arithmetic; `Value` with navigation that
  never fails, the readers, JSON Pointer and iterators; `Entry`, `Kind`,
  `ParseError`, and the `Parser` and `DataWriter` interfaces;
- `zink.json`: `JsonReader`, a state machine with its own stack, so nesting can't
  overflow the native one; `JsonParser`, built on its events; `JsonWriter` and
  `json.write`. Strings and numbers are views into the input unless a string has
  escapes. `jsonfmt` matches Python's `json` byte for byte on random documents, and
  accepts and rejects one-character mutations of them as Python's strict parser
  does; it re-indents 48 MB in 0.35 s streaming and 0.65 s through a tree with keys
  sorted, against 3.2 s for Python and 2.2 s for `jq`.

`zink.lang`'s `Iterator<T>` is in `lib/zink/lang/`, with every library iterator,
each implementing it: `LineIterator`, `WordIterator`, `SplitIterator` and
`CodePointIterator` for text, `ChunkIterator<T>` and `WindowIterator<T>` for arrays,
and `zink.cli`'s `ArgIterator`. `text.lines()` calls the package-private
`LineIterator.lines(text)`, so an iterator can be kept in a variable, stepped with
`next()`, given to generic code or passed as an `Iterator<string>&`. A for-each over
`lines()`, `words()`, `splitBy()` and `codePoints()` on 26 MB of text takes 0.20 s,
against 0.25 s for the C functions they replace, since each `next()` is a direct
call the C compiler inlines. `repeat` and `split` are Zinkite too, in `TextMethods`:
`split` builds its array with `splitBy`, where the caller keeps it, of views into the
text.

Where they differ from STDLIB: `fs.move` of a directory across file systems still
fails with `CROSS_DEVICE`; `hash.xxh3`, JSON5 and `TreeBuilder` aren't written.

Not yet: a lambda that captures, passed to a method called through an interface
or to a constructor; comparing class values; and the library packages that
aren't written yet (`zink.toml`, `zink.regex` and the others).
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
| `src/codegen.rs` | the C backend, with the copy of a function's base case that its calls to itself make (DESIGN §14.2) |
| `src/codegen/` | its parts for lambdas, hash maps, sorting, generics, interfaces and threads, and `versions.rs`, which runs counted loops without their index checks when a test before them shows every index in range (DESIGN §14.2) |
| `src/cheader.rs` | reading C headers with clang, and caching what it finds |
| `src/driver.rs` | compiling the C, with jumps kept off 32-byte boundaries on x86-64 (DESIGN §12.5), the cache, and running the result |
| `src/stdlib.rs` | the standard library's files, built into `zink`, and which a program needs |
| `lib/zink/` | the standard library, in Zinkite: `io/`, `cli/` and `lang/` so far |
| `runtime/zink_rt.h` | the runtime: data stacks, output, failures, checked arithmetic, text, and the choice of a `simd` function's version (DESIGN §12.8) |
| `src/main.rs` | the `zink` command |
| `tests/parse.rs` | parser tests |
| `tests/check.rs` | type checker tests |
| `tests/lifetimes.rs` | lifetime tests, from `REFERENCES.md` §10 |
| `tests/run.rs` | programs compiled to C, built and run |
