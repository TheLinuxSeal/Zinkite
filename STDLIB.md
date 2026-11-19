# Zinkite Standard Library (draft)

> Companion to `DESIGN.md`. Lists what the standard library contains, with a signature and the status codes for each function. It keeps to the language's rules: no hidden memory (DESIGN §3), views rooted at their inputs (`REFERENCES.md`), and a `u8` status per call (DESIGN §7). Everything here is **Proposed** unless marked otherwise.
> **Updated:** 2026-11-19. First version: what CLI tools need, plus JSON, JSON5, TOML and YAML.

---

## 1. Overview

### 1.1 Packages

| Package | Contents | Tier | § |
|---|---|---|---|
| `zink.lang` | imported automatically: output and streams, `status`, `exit`, `fail`, `env`, number parsing and helpers, text methods, array methods, `hashmap`, `hashset`, `pool`, `Iterator` | v1 | 2, 3, 7 |
| `zink.io` | `File`, `Dir`, `fs`, `path` | v1 | 4 |
| `zink.os` | `process`, `Command`, `term` | v1 | 5.1–5.2 |
| `zink.time` | `time`, `Duration`, `Instant`, `Timestamp`, `DateTime` | v1 | 5.3 |
| `zink.cli` | `Args`, `ansi` | v1 | 6 |
| `zink.math` | `math` | v1 | 8.1 |
| `zink.random` | `random`, `Rng` | v1 | 8.2 |
| `zink.hash` | `hash`, `Crc32`, `Sha256` | v1 | 8.3 |
| `zink.encoding` | `hex`, `base64` | v1 | 8.4 |
| `zink.csv` | `CsvReader`, `CsvWriter` | v1 | 9.1 |
| `zink.data` | the shared parsed tree: `Node`, `Value`, `Entry`, `Kind`, `ParseError`, `Parser`, `DataWriter`, `TreeBuilder` | later | 9.2, 9.7 |
| `zink.json` | `JsonParser`, `JsonReader`, `JsonWriter`, `json`; JSON and JSON5 | later | 9.3, 9.4 |
| `zink.toml` | `TomlParser`, `toml` | later | 9.5 |
| `zink.yaml` | `YamlParser`, `yaml` | later | 9.6 |
| `zink.glob`, `zink.regex` | `glob`, `Regex` | later | 9.9 |
| `zink.net`, `zink.compress`, `zink.thread`, `zink.bigint`, `zink.test` | networking, gzip, workers, big integers, tests | later | 10 |
| `vendor.glfw`, `vendor.sdl3`, `vendor.gl`, `vendor.vulkan` | windows, input and the GPU, through GLFW, SDL3, OpenGL and Vulkan; separate packages, not the standard library | later | 10.6 |

- **v1** is what the first release ships: enough for the four example programs and for typical CLI tools.
- **Later** comes after v1. It is designed here so that v1 doesn't paint it into a corner.
- **`zink.*` is reserved** for the standard library (DESIGN §8.4).
- **A large library doesn't make programs larger.** Dead-code elimination drops whatever a program doesn't call, including the Unicode tables and the YAML parser.

### 1.2 Naming
- **Classes whose values a program holds are capitalized**: `File`, `Rng`, `JsonParser`.
- **Built-in types are lowercase**, as `string` and `hashmap` already are: `hashset`, `pool`. So are the built-in generic bounds: `number`, `ordered`, `hashable` and the rest (DESIGN §10.1).
- **Classes with only static members are lowercase**, as `status` already is: `math.sqrt(x)`, `path.join(a, b)`, `fs.remove(p)`.
  - They read as namespaces.
  - They never collide with a program's own capitalized classes.
- **Format-specific names carry the format**: `JsonParser`, `TomlParser`. Otherwise two wildcard imports would bring in two `Parser`s (DESIGN §8.5).
- **Overloads share a name when they do the same thing for different inputs** (DESIGN §6.5): `i64.parse(s)` and `i64.parse(s, 16)`, `s.find(x)` and `s.find(x, from)`, and a writer's one `write`, for `string`, `i64`, `f64` and the rest.
- **Different behaviour gets a different name**, even where the types would tell the calls apart: `sort(less)` and `sortBy(key)`, `parse` and `parsePrefix`.

### 1.3 Conventions
These follow from the memory model and the status code. Every section below keeps to them.

1. **Text that is read goes into a buffer the caller passes.** The parameter is a `u8[..>]&`, and the text is appended at the buffer's end (`stdin.readLine(line)`, `File.readAll(path, text)`).
   - The caller decides how long the text lives, and can reuse the buffer by clearing it.
   - Such a parameter accepts any `u8` array that can grow at its end, a `u8[..~]` included (DESIGN §6.1).
2. **Text that is computed is returned as a `u8[.]`, built in place** where the caller keeps it (DESIGN §3.3).
   - `u8[.] up = name.toUpper();` writes the text once, into `up`.
   - Passed straight to a `string` parameter, the result is a statement temporary (DESIGN §4.11).
3. **Small fixed-size results are returned by value**, in capped arrays, and never touch a data stack. `n.hex()` is a `u8[..~16]`, and `hash.sha256(b)` is a `u8[.32]`.
4. **Views point into the input.** These are all views into their argument, rooted there (REFERENCES §4), so nothing is copied:
   - `s.trim()`, `s.split(",")` and `path.base(p)`;
   - every parsed string that has no escapes.
5. **Anything that walks over data is a lazy iterator** (DESIGN §9.4): `lines()`, `splitBy()`, `codePoints()`, `fs.walk()`, `JsonReader`. An eager form exists where an array is often wanted (`split`).
   - Readers that hand out one item per call are looped over with a repeat loop: `while (DirEntry e : d.next(names))`, `while (string line : stdin.nextLine(buf))`.
6. **Every failure is a status code** (DESIGN §7.7).
   - Operating-system errors pass through as their `errno` number.
   - Each signature lists the codes worth handling. A call that wraps a system call can also fail with any other `errno` the kernel returns.
   - The value on failure is the zero value (DESIGN §7.5). The `...Or(fallback)` functions are the one documented exception, as `getOr` already is (DESIGN §4.10).
7. **A function that can't fail sets no status**, so calling it never draws the dropped-status warning (DESIGN §7.3). Writers set none per call and report once, at `finish()` (§9.7).
8. **Misuse is a bug and stops the program** (DESIGN §7.6), as an index out of range does. Examples: `w.key("a")` outside an object, `rng.range(5, 1)`, or declaring a 65th option. Expected failures, such as a missing file or bad input, are statuses.
9. **Lambdas are compile-time values** (DESIGN §9.3), so `filter`, `sortBy` and `retain` compile to the loop you would write by hand.
10. **Parsers report where input went wrong.** A status can't carry a line number, so each parser and reader keeps a `ParseError` with the line, the column and a message, to read after a failure (§9.2).
11. **Library objects that write into a caller's buffer hold a reference to it**: `JsonWriter`, `CsvWriter`, `TreeBuilder`, `Walker`. The reference is to the array itself, not into its elements, so it may be a field (REFERENCES §7.1).
12. **Signatures are written as declarations**, with the receiver as a variable: `i64 s.find(string needle)` means a method on a `string` `s`. A comment lists the statuses it can fail with.

### 1.4 Status codes the library adds
DESIGN §7.7 lists them too.

| Name | Number | Source | Description | Used by |
|---|---|---|---|---|
| `CROSS_DEVICE` | 18 | `EXDEV` | Invalid cross-device link | `fs.rename` across file systems (`fs.move` copies instead) |
| `NOT_TERMINAL` | 25 | `ENOTTY` | Inappropriate ioctl for device | `term.size()` when stdout isn't a terminal |
| `NOT_EMPTY` | 39 | `ENOTEMPTY` | Directory not empty | `fs.remove` on a directory that has entries |
| `UNSUPPORTED` | 95 | `EOPNOTSUPP` | Operation not supported | input that uses a feature a parser leaves out, such as YAML keys that are collections |
| `TYPE` | 197 | Zinkite | Wrong kind of value: `asI64()` on a string | tree accessors (§9.2), `finish()` on writers (§9.7) |
| `NO_MEMORY` | 12 | `ENOMEM` | Cannot allocate memory | the graphics bindings, when host or GPU memory runs out (§10.6) |
| `GPU` | 198 | Zinkite | A graphics API call failed; the binding gives the exact code | `vendor.gl`, `vendor.vulkan` (§10.6) |
| `OUT_OF_DATE` | 199 | Zinkite | The window's surface changed, so the swapchain must be rebuilt | `vendor.vulkan` (§10.6) |

- **Malformed input fails with `INVALID`**: bad JSON, an unterminated CSV quote, a bad glob pattern. `i64.parse("abc")` already does the same. The parser's `ParseError` says where.
- **Exceeding a nesting limit fails with `FULL`**, the same code as a push on a full capped array.

---

## 2. Core: `zink.lang` (v1)

Imported into every file.

### 2.1 Output and streams
Decided in DESIGN §4.13; listed here so the library is in one place.

```
void print(string text)          // to stdout
void println(string text)        // adds a newline
void errprint(string text)       // to stderr
void errprintln(string text)
```

- **They set no status** (DESIGN §4.13), so a bare `println` draws no warning. A failed write is reported on stderr at exit and makes the exit status 1. For code that must check each write, `stdout.write(...)` and `stdout.flush()` fail with `IO`, `NO_SPACE` or `BROKEN_PIPE`.
- **Writing to stdout after its reader has gone ends the program quietly**, so `BROKEN_PIPE` is only reported for stderr (DESIGN §7.6).
- **`stdin`, `stdout` and `stderr` are `File` handles** for descriptors 0, 1 and 2, with the methods of §4.1 (Proposed, §11). DESIGN §4.13 only says they have "the same methods as files".
- **Buffering**:
  - stdout is line-buffered on a terminal and block-buffered otherwise;
  - stderr is unbuffered;
  - `stdout.flush()` writes whatever is pending;
  - everything is flushed at exit (DESIGN §6.4).

### 2.2 Status, exit and failure

```
string status.message(u8 code)   // "No such file or directory"; "Error 230" for a program's own codes
string status.name(u8 code)      // "NOT_FOUND"; the C name ("ESRCH") for errno codes without a readable one; "" for 224–255
void exit(u8 code)               // DESIGN §8.6: files flushed and closed, defers don't run
void fail(string message)        // stops the program as a bug does
```

- **Named codes** are constants on `status`: `status.OK`, `status.FAILED`, `status.NOT_FOUND`, … (DESIGN §7.7, and §1.4 here).
- **`fail` is for states the program believes impossible**, such as `fail("unreachable: kind ${k}")`.
  - It prints the message and the source location on stderr, flushes and closes files, and exits with 1, exactly like an index out of range (DESIGN §7.6).
  - Nothing can catch it.
- **The compiler knows that `exit` and `fail` never return**, so nothing is needed after them: no `return`, and no value for a function's result.

### 2.3 Environment: `env`

```
string env.get(string name)               // MISSING
bool env.has(string name)
void env.set(string name, string value)   // INVALID: name empty or containing '='
void env.unset(string name)
EnvIterator env.all()                     // for (EnvVar v : env.all()) { ... v.name ... v.value ... }
```

- **Views from `get` are rooted at `static`.** The runtime never frees environment text, even after `set` replaces a variable, so an earlier view stays valid and keeps the old value.
- **`set` and `unset` change what child processes inherit** (§5.1).

### 2.4 Numbers
On every number type `T`:

```
T T.parse(string s)                   // INVALID, RANGE
T T.parse(string s, i64 base)         // integer types; base 2–36, no prefix; INVALID, RANGE
(T, string) T.parsePrefix(string s)   // the number at the start of s, and the rest of s; INVALID, RANGE

T.MIN, T.MAX                          // integer types
f64.INFINITY, f64.NAN, f64.EPSILON    // and the same on f32
bool x.isNaN(), bool x.isFinite()     // float types

<S implements number> T T.from(S x)   // what (T) x does, but RANGE instead of stopping

<T implements ordered> T min(T a, T b)
<T implements ordered> T max(T a, T b)
<T implements ordered> T clamp(T x, T lo, T hi)    // lo > hi is a bug
<T implements number> T abs(T x)                   // abs(i64.MIN) overflows, so it stops the program
<T implements int> T x.divOr(T d, T fallback)     // x / d, or fallback when d is 0
```

- **`parse` accepts** a sign, then digits, with no spaces and no `_`.
  - Unsigned types take no `-`.
  - `f64.parse` also takes a fraction, an exponent, `inf` and `nan`.
- **`parsePrefix` is for tokenizers**: `(i64 n, string rest) @bad = i64.parsePrefix("42px");` gives 42 and `"px"`.
- **Floats round-trip.** `f64.parse` rounds correctly, and `${x}` prints the shortest text that parses back to the same value, so `0.1` prints as `0.1`.
- **Formatting methods** are in DESIGN §4.13: `hex()`, `bin()`, `pad(w)`, `zeroPad(w)`, `fixed(d)`, `utf8()`. The library adds two:
  - `oct()`, in base 8;
  - `sci(d)`, scientific notation with `d` digits after the point (`1.50e3`).
- **`T.from(x)` converts with a check that reports.** `u16 port @bad = u16.from(n);` gives 0 with `RANGE` when `n` doesn't fit, where `(u16) n` would stop the program (DESIGN §9.1). From a float it truncates toward zero, as the cast does, and NaN fails with `RANGE`.
- **`min`, `max` and `clamp` take any `ordered` type**, strings included, with both arguments of the same type. On floats, NaN counts as the largest value (DESIGN §10.1).
- **Bit operations** are in §8.1.

### 2.5 Iterators
```
interface Iterator<T> { mut T next(); }      // DESIGN §9.4 (Decided)
```

Every library iterator implements it. The library's iterator classes (`LineIterator`, `SplitIterator`, …) are named in the signatures but are rarely written out, since they are usually looped over directly: `for (string l : text.lines())`.

To steer an iterator from inside the loop, or to read its state afterwards, declare it and call `next()` in a repeat loop (DESIGN §9.4): `while (WalkEntry e : w.next())`.

---

## 3. Text (v1, in `zink.lang`)

- **Text methods take their receiver as a `string`**, a view (DESIGN §4.11).
- **They are functions called with method syntax** (DESIGN §6.1).
- **On a buffer, call them directly**: `line.trim()` works on a `u8` array, which converts to a view of its bytes (DESIGN §4.10).

### 3.1 Searching
```
i64 s.find(string needle)                  // MISSING
i64 s.find(string needle, i64 start)       // MISSING; start > s.len is a bug
i64 s.rfind(string needle)                 // MISSING
i64 s.findAny(string bytes)                // the first byte that is one of bytes; MISSING
bool s.contains(string needle)
bool s.startsWith(string prefix)
bool s.endsWith(string suffix)
i64 s.count(string needle)                 // non-overlapping occurrences
```

- **Needles and separators are strings**: `s.find(",")`. A character literal is an integer (DESIGN §9.1), so `s.find(',')` is a type error, and the message suggests `","`.
- **Searching is linear time**: a `memchr`-style scan for one byte, and the two-way algorithm for longer needles.

### 3.2 Splitting and joining
```
string[..>] s.split(string sep)              // eager: an array of views
SplitIterator s.splitBy(string sep)          // lazy: for (string part : s.splitBy(",")) { ... }
LineIterator s.lines()                       // lines end in \n or \r\n
WordIterator s.words()                       // separated by runs of ASCII whitespace
(string, string) s.splitFirst(string sep)    // before and after the first sep
(string, string) s.splitLast(string sep)     // before and after the last sep
u8[.] parts.join(string sep)                 // on any array or slice of strings
```

- **Every part is a view into `s`.**
- **`split` and `splitBy` keep empty parts**: `"a,,b".split(",")` is `["a", "", "b"]`, and `"".split(",")` is `[""]`.
- **`lines()` yields no empty last line** after a final newline. **`words()` never yields an empty word.**
- **`splitFirst` and `splitLast` never fail.** With no separator they give `(s, "")`:
  - `"home".splitFirst(" ")` is `("home", "")`;
  - `"file.tar.gz".splitLast(".")` is `("file.tar", "gz")`, and `"README".splitLast(".")` is `("README", "")`.
  - `linebuf.zn`'s own `splitFirst` becomes `line.splitFirst(" ")`. Use `contains` first when the difference between `"home"` and `"home "` matters.

### 3.3 Trimming and comparing
```
string s.trim()                        // ASCII whitespace at both ends
string s.trimStart()
string s.trimEnd()
string s.trimPrefix(string prefix)     // s unchanged when it doesn't start with prefix
string s.trimSuffix(string suffix)
i32 s.compareTo(string other)          // byte order: below 0, 0 or above 0
i32 s.compareNatural(string other)     // runs of digits compare as numbers: "file2" < "file10"
bool s.equalsIgnoreCase(string other)  // ASCII letters only
```

- `==` compares contents (DESIGN §4.11).
- **`<`, `<=`, `>` and `>=` compare strings in the same byte order**, since `string` is `ordered` (DESIGN §10.1): `if (a.name < b.name)`.
- **`compareNatural`** compares runs of ASCII digits by their value and everything else byte by byte, as `ls -v` does: `"file2"` comes before `"file10"`, and `"v1.9"` before `"v1.10"`. Equal numbers written differently, such as `01` and `1`, fall back to byte order.

### 3.4 Transforming
```
u8[.] s.toUpper()                           // ASCII letters; every other byte unchanged
u8[.] s.toLower()
u8[.] s.replaceAll(string from, string to)  // from == "" is a bug
u8[.] s.replaceFirst(string from, string to)
u8[.] s.repeat(i64 n)
u8[.] s.pad(i64 w)                          // right-aligned in w code points, as pad(w) on numbers
u8[.] s.padEnd(i64 w)                       // left-aligned
```

- **Each returns new text, built in place** (§1.3, rule 2): `u8[.] key = word.toLower();`.
- **Case mapping beyond ASCII comes later**, with the Unicode tables (§3.5).

### 3.5 UTF-8 and bytes
```
CodePointIterator s.codePoints()   // decodes; an invalid sequence gives U+FFFD and the loop goes on
bool s.isValidUtf8()
i64 s.codePointCount()
```

On `u8`, ASCII only:
```
bool b.isDigit()    bool b.isHexDigit()    bool b.isAlpha()    bool b.isAlnum()
bool b.isSpace()    bool b.isUpper()       bool b.isLower()    bool b.isPrint()
u8 b.toUpper()      u8 b.toLower()         // other bytes unchanged
```

- `wordfreq.zn`'s `lower()` becomes `b.toLower()`.
- **Unicode properties come later**: `isAlphabetic`, `isWhitespace` and case mapping on `u32`, plus the display width of text in terminal columns. They need tables, and which Unicode version to ship is open (§11).

### 3.6 Building text
On every `u8` array that can grow at the end:

```
void b.append(string s)                          // FULL on a capped array without room
void b.appendUtf8(u32[:] text, i64 from, i64 to) // encodes text[from:to] as UTF-8
b +> 'x'                                         // one byte (DESIGN §4.8)
```

- **`append` of interpolated text writes the parts directly**, with no temporary (DESIGN §4.11).
- **`appendUtf8` takes a slice**, so a cursor array can't be passed to it until slices of cursor arrays are decided (DESIGN §17). Until then `linebuf.zn`'s `show` encodes one code point at a time.

---

## 4. Files and the filesystem: `zink.io` (v1)

### 4.1 `File`
A `File` is a handle (DESIGN §6.4).

| Call | Opens the file | Fails with |
|---|---|---|
| `File.open(string path)` | for reading | `NOT_FOUND`, `PERMISSION` |
| `File.create(string path)` | for writing, created or truncated; mode 0666 less the umask | `NOT_FOUND` (a missing directory), `PERMISSION`, `IS_DIRECTORY` |
| `File.append(string path)` | for writing at the end, created if missing | as `create` |
| `File.createNew(string path)` | for writing; it must not exist yet (`O_EXCL`), so it works as a lock file | `EXISTS`, and as `create` |
| `File.update(string path)` | for reading and writing, without truncating | `NOT_FOUND`, `PERMISSION` |

Reading appends to the buffer:
```
i64 f.read(u8[..>]& buf, i64 max)     // up to max bytes; returns how many; END at the end of the file
void f.readLine(u8[..>]& buf)         // one line, without its \n or \r\n; END when nothing was left
string f.nextLine(u8[..~]& buf)       // clears buf, reads one line into it, and returns a view of it; END as readLine
void f.readAll(u8[..>]& buf)          // to the end of the file
```

Writing:
```
void f.write(string bytes)            // IO, NO_SPACE, BROKEN_PIPE, BAD_HANDLE
void f.print(string text)
void f.println(string text)
void f.flush()
void f.sync()                         // flush, then fsync
void f.close()                        // flushes first; BAD_HANDLE when already closed
```

Position and type:
```
void f.seek(i64 pos)                  // from the start; ESPIPE on a pipe
i64 f.position()
i64 f.size()
bool f.isTerminal()
```

Whole files in one call:
```
void File.readAll(string path, u8[..>]& buf)    // open, read to the end, close
void File.writeAll(string path, string bytes)   // replaces the file atomically
void File.appendAll(string path, string bytes)
```

- **`readLine` keeps a last line that has no newline**, and fails with `END` only when nothing at all was left.
- **`nextLine` is for repeat loops**: `while (string line : stdin.nextLine(buf)) { ... }` reads every line into one reused buffer. Clearing it is allowed, because the previous pass's `line` is gone when the next call runs (REFERENCES §9).
- **`writeAll` writes a temporary file next to the target, then renames it over the target.**
  - Another process sees the old contents or the new ones, never half a file.
  - The old file's permissions are kept.
  - A symbolic link is followed, so its target is the file that gets replaced.

### 4.2 Directories: `Dir`
```
Dir Dir.open(string path)             // NOT_FOUND, PERMISSION, NOT_DIRECTORY
DirEntry d.next(u8[..>]& names)       // END after the last entry; the name is appended to names
void d.close()                        // BAD_HANDLE when already closed

class DirEntry { string name; bool isDir; bool isLink; i64 size; }
```

- **Loop over the entries** with `while (DirEntry e : d.next(names)) { ... }` (DESIGN §9.4).
- **`.` and `..` are skipped.** `dirwalk.zn` still checks for them, which is now harmless.
- **`isDir` and `isLink` come free** from the directory read on most file systems. `size` costs one `fstatat` per entry.
- **Order is the file system's**, as `ls -U` shows it. Sort the names if order matters.

### 4.3 Walking a tree: `fs.walk`
```
Walker fs.walk(string root, u8[..~]& path)
WalkEntry w.next()        // END after the last entry
void w.skip()             // don't descend into the directory just returned

class WalkEntry { string path; string name; i64 depth; bool isDir; bool isLink; i64 size; u8 err; }
```

```
u8[..~] buf = [];
Walker w = fs.walk("src", buf);
while (WalkEntry e : w.next()) {
    if (e.isDir && e.name == ".git") { w.skip(); continue; }
    if (e.err) {
        errprintln("can't read ${e.path}: ${status.message(e.err)}");
        continue;
    }
    if (e.path.endsWith(".zn")) { println(e.path); }
}
```

- **The walk is depth-first**, and each directory comes before its contents.
- **`path` holds only the current path.**
  - The walker appends each name and removes it again, so the buffer never grows past the deepest path.
  - `e.path` is a view into the buffer and stays valid until the next step.
  - Keeping it longer is a compile error under rule R, and the message suggests copying it (`kept.append(e.path)`).
- **A directory that can't be read is still returned**, with `err` set, and the walk goes on without descending into it.
- **Symbolic links aren't followed** unless `w.followLinks` is set. When they are, a loop is detected by device and inode, and returned with `err` set to `status.ELOOP`.
- **Fields**: `bool followLinks = false; i64 maxDepth = i64.MAX;`. The walker keeps one open directory per level, in its one growable field.
- **`w.skip()` acts on the walker the loop calls**, since the repeat loop names it (DESIGN §9.4). `for (WalkEntry e : w)` is an error that suggests this form.

### 4.4 File system operations: `fs`
```
bool fs.exists(string path)
Stat fs.stat(string path)                // NOT_FOUND, PERMISSION; follows symbolic links
Stat fs.lstat(string path)               // doesn't follow them
void fs.remove(string path)              // a file, a link or an empty directory; NOT_FOUND, NOT_EMPTY, PERMISSION
void fs.removeAll(string path)           // recursively, never following links; a missing path is success
void fs.rename(string from, string to)   // atomic; CROSS_DEVICE across file systems
void fs.move(string from, string to)     // rename, or copy and remove across file systems
void fs.copy(string from, string to)     // contents and mode; copy_file_range where it can
void fs.mkdir(string path)               // EXISTS, NOT_FOUND (a missing parent)
void fs.mkdirAll(string path)            // creates missing parents; an existing directory is success
void fs.symlink(string target, string link)
void fs.readLink(string path, u8[..>]& buf)
void fs.chmod(string path, u32 mode)
File fs.tempFile(u8[..>]& path)          // a new file in $TMPDIR, or /tmp; its path is appended to path
void fs.tempDir(u8[..>]& path)           // a new directory there, mode 0700

class Stat { i64 size; Timestamp modified; u32 mode; u32 uid; u32 gid; bool isDir; bool isFile; bool isLink; }
```

- **Temporary names are random** (from `getrandom`) and created with `O_EXCL`.
- **Removing temporary files is the caller's job**: `defer @_ = fs.remove(tmp);`.

### 4.5 Paths: `path`
```
u8[.] path.join(string a, string b)        // "a/b"; an absolute b replaces a
string path.base(string p)                 // "dir/file.txt" → "file.txt"; "a/" → "a"; "/" → "/"
string path.dir(string p)                  // "dir/file.txt" → "dir"; "file.txt" → "."
string path.ext(string p)                  // "file.tar.gz" → ".gz"; ".bashrc" → ""
string path.stem(string p)                 // "file.tar.gz" → "file.tar"
bool path.isAbsolute(string p)
u8[.] path.normalize(string p)             // removes ".", "..", repeated and trailing slashes
u8[.] path.absolute(string p)              // joined to the working directory, then normalized
u8[.] path.relative(string from, string to)
u8[.] path.resolve(string p)               // follows symbolic links (realpath); NOT_FOUND
void path.cwd(u8[..>]& buf)
void path.chdir(string p)                  // NOT_FOUND, NOT_DIRECTORY, PERMISSION
```

- **Paths are bytes**, as on Linux, and don't need to be valid UTF-8.
- **Everything except `resolve`, `cwd` and `chdir` is lexical** and never touches the file system. So `normalize("a/../..")` is `".."` and `normalize("/..")` is `"/"`.
- **`base`, `dir`, `ext` and `stem` return views** into `p` (or a literal, for `"."`).

---

## 5. Processes, the terminal and time

### 5.1 Running programs: `process` (v1, `zink.os`)
```
Exit process.run(string program, string[:] args, u8[..>]& out)   // NOT_FOUND, PERMISSION

class Exit { u8 code; u8 signal; }
bool e.ok()                       // code 0 and no signal
```

```
u8[..>] out = [];
Exit e @err = process.run("git", ["rev-parse", "HEAD"], out);
if (err) {
    errprintln("can't run git: ${status.message(err)}");
    return err;
}
if (!e.ok()) { return e.code; }
string commit = out.trim();
```

- **A program without a `/` is searched for in `PATH`.** Arguments reach it as they are, with no shell in between, so nothing needs quoting.
- **The child's stdout is appended to `out`.** Its stdin and stderr are this program's.
- **A program that runs and fails isn't a failed call.** The status is 0, and `e.code` (or `e.signal`, when it was killed) says how it ended. The status is nonzero only when the program couldn't be started.

`Command` gives more control:
```
Command c = new Command("tar");
c.arg("-czf");
c.arg(archive);
c.args(files);
c.cwd = "build";
c.setEnv("LC_ALL", "C");
c.input = manifest;              // fed to its stdin; without it, stdin is inherited
c.timeout = Duration.seconds(30);    // after that it is killed, and the call fails with TIMEOUT
```

- **Storage**: the arguments and the environment changes share `Command`'s one growable field, a list of strings. `cwd` and `input` are views, so what they view must outlive the command (rule S).

```
Exit c.run()                                    // stdout and stderr inherited
Exit c.output(u8[..>]& out)                     // stdout appended to out
Exit c.outputBoth(u8[..>]& out, u8[..>]& errs)  // stdout and stderr captured separately
Child c.spawn()                                 // runs alongside this program
Exit ch.wait()
void ch.kill()                                  // SIGTERM; ch.killNow() sends SIGKILL
```

Other functions:
```
void process.exec(string program, string[:] args)   // replaces this process; returns only on failure
i32 process.pid()
string process.name()                               // argv[0]; main's args don't include it
u8[.] process.which(string program)                 // the full path PATH finds; NOT_FOUND
```

- **Later**: pipelines (`a | b`), pipes to and from a spawned child, and signal handlers, such as cleanup on Ctrl-C.

### 5.2 The terminal: `term` (v1, `zink.os`)
```
bool f.isTerminal()                // on every File, stdout included (§4.1)
(i64, i64) term.size()             // (columns, rows) of stdout's terminal; NOT_TERMINAL
```

- `(i64 cols, _) @notTty = term.size();` gets the width alone.
- **Colors** are in `ansi` (§6.2).
- **Later**: raw mode, reading keys, and reading a password without echo.

### 5.3 Time: `zink.time` (v1)
```
Timestamp time.now()                 // wall clock: nanoseconds since 1970-01-01 UTC
Instant time.monotonic()             // for measuring; never goes backwards
void time.sleep(Duration d)
```

`Duration`:
```
class Duration { i64 nanos; }
Duration Duration.ns(i64 n)          // also .us, .ms, .seconds, .minutes, .hours
i64 d.toMs()
f64 d.toSeconds()
Duration d.plus(Duration other)
Duration d.minus(Duration other)
u8[..~24] d.human()                  // "350ms", "1.25s", "2h03m"
```

`Instant`, for measuring:
```
Duration i.elapsed()                 // since i
Duration a.since(Instant b)
```

`Timestamp`, a point in wall-clock time:
```
Timestamp Timestamp.fromUnix(i64 seconds)
i64 t.unixSeconds()
DateTime t.utc()
u8[..~32] t.iso()                    // "2026-11-18T09:30:00Z", with fractional seconds when nonzero
```

`DateTime`, calendar fields:
```
class DateTime { i32 year; u8 month; u8 day; u8 hour; u8 minute; u8 second; u32 nanos; i16 offsetMinutes; u8 parts; }
DateTime DateTime.parse(string s)    // RFC 3339, plus its date-only and time-only forms; INVALID
u8[..~40] dt.iso()
Timestamp dt.toTimestamp()           // INVALID without a date or an offset
u8 dt.weekday()                      // 1 = Monday … 7 = Sunday, as in ISO 8601
```

- **`parts` says which pieces the text had**: a date, a time, an offset. TOML's local dates and times (§9.5) need it.
- **Later**:
  - time zones beyond UTC and fixed offsets (reading `/etc/localtime` and the tz database);
  - formatting with patterns.

---

## 6. Command-line tools: `zink.cli` (v1)

### 6.1 Arguments: `Args`
```
Args cli = new Args("csvsort", "Sort CSV rows by one column.");
cli.flag('n', "numeric", "compare the column as numbers");
cli.flag('r', "reverse", "sort in descending order");
cli.option('k', "column", "N", "sort by column N, counting from 1");
cli.positional("file", "input file; stdin when left out");
cli.parseOrExit(args);

bool numeric = cli.has("numeric");
i64 column @bad = cli.getI64("column", 1);
if (bad || column < 1) { cli.usageError("--column takes a number from 1"); }
string file = cli.get("file", "");
```

The generated `--help`:
```
usage: csvsort [-n] [-r] [-k N] [file]

Sort CSV rows by one column.

  -n, --numeric     compare the column as numbers
  -r, --reverse     sort in descending order
  -k, --column N    sort by column N, counting from 1
  -h, --help        show this help and exit
  file              input file; stdin when left out
```

Declaring:
```
void cli.flag(u8 letter, string name, string help)                // letter 0 for none
void cli.option(u8 letter, string name, string meta, string help)
void cli.positional(string name, string help)                     // optional, in order
void cli.rest(string name, string help)                           // any number more: [files...]
cli.version = "1.2.0";                                            // adds --version
```

Parsing:
```
void cli.parse(string[:] args)         // INVALID; cli.error() says what was wrong
void cli.parseOrExit(string[:] args)   // --help and --version print and exit 0; a usage error exits 2
void cli.usageError(string message)    // prints "csvsort: message" and the usage line, then exits 2
```

Reading:
```
bool cli.has(string name)
i64 cli.count(string name)                     // -vvv gives 3
string cli.get(string name, string fallback)
i64 cli.getI64(string name, i64 fallback)      // INVALID, RANGE
f64 cli.getF64(string name, f64 fallback)      // INVALID
ArgIterator cli.getAll(string name)            // every occurrence: -I a -I b
ArgIterator cli.restValues()
```

- **Accepted forms**:
  - flags alone or combined (`-n`, `-nr`);
  - values after a space, attached, or after `=` (`-k 3`, `-k3`, `--column 3`, `--column=3`);
  - `--`, which ends the options.
- **Usage errors**: an unknown option, a missing value, or an extra argument. Exit status 2 for them follows getopt-based tools and Python's argparse.
- **The fallback is used only when the option is absent.** A value that doesn't parse gives the zero value with `INVALID`.
- **Values are views into `args`**, so nothing is copied, and `cli` can't outlive `args`.
- **Storage**: declared options sit in a capped table inside `Args`, so a 65th option is a bug. The values found go in its one growable field.
- **Options are looked up by their long name.** Asking for one that was never declared is a bug, which catches a typo the first time that line runs.
- **Subcommands** (`tool add …`) are later. Until then, switch on the first positional and give the rest (`restValues()`) to a second `Args`.

### 6.2 Colors: `ansi`
```
Colors ansi.colors(File f)     // escape codes when f is a terminal, NO_COLOR is unset and TERM isn't "dumb"
Colors ansi.always()           // for --color=always
Colors ansi.never()            // for --color=never

class Colors { string bold; string dim; string underline; string red; string green; string yellow;
               string blue; string magenta; string cyan; string gray; string reset; }
```

```
Colors c = ansi.colors(stderr);
errprintln("${c.red}${c.bold}error:${c.reset} can't open ${path}");
```

- **When colors are off, every field is `""`**, so the same code prints plain text into a pipe.
- **The strings are static**, so a `Colors` is a handful of views.

---

## 7. Collections and algorithms

### 7.1 Methods on arrays (v1)
These work on every array kind, specialized per layout (DESIGN §10). "Needs" says what the array's type must allow. The compiler rejects the method otherwise, as it does for `push` (DESIGN §4.7).

**Bounds** (DESIGN §10.1): methods that order elements in their natural order (the sorts without a comparator, `binarySearch`, `lowerBound`, `upperBound`, `min()` and `max()`) need `T implements ordered`, the key of a `By` method must be `ordered`, and `sum()` needs `number`. `radixSort` takes only numbers and strings, as keys or as elements. The rest work on any fixed-size element type, and `indexOf`, `contains` and `dedup` compare with `==`. On floats, the ordering methods put NaN last.

Methods that change the array need fixed-size elements, and an array or a writable view (`T[:]&`, DESIGN §4.10) rather than a read-only slice: `rows[1:].sort()` sorts all but the first row.

**Sorting.** Every sort works in place, on an array or a writable view, with fixed-size elements. Most come in three forms (DESIGN §6.5):
- **`()`** uses `T`'s natural order, and needs `T implements ordered`;
- **`(less)`** takes a comparator, a `func(T, T => bool)` that says whether its first argument goes before its second;
- **`By(key)`** takes a key function, a `func(T => K)` with `K implements ordered`, and compares the keys in their natural order.

| Methods | Algorithm | Time (worst case) | Scratch space | Stable |
|---|---|---|---|---|
| `sort()`, `sort(less)`, `sortBy(key)` | pattern-defeating quicksort, falling back to heapsort | O(n log n) | none | no |
| `sortDescending()`, `sortByDescending(key)` | the same, largest first | O(n log n) | none | no |
| `stableSort()`, `stableSort(less)`, `stableSortBy(key)` | merge sort that reuses the runs already in the input | O(n log n) | n/2 elements | yes |
| `radixSort()`, `radixSortBy(key)` | radix sort: least significant byte first (LSD) for number keys, most significant first (MSD) for strings | numbers: O(n × key size in bytes); strings: O(the bytes needed to tell the keys apart) | n elements; for strings, also a work list of buckets | yes |
| `partialSort(k)`, `partialSort(k, less)`, `partialSortBy(k, key)` | selection, then a sort of the first `k` | O(n + k log k) | none | no |
| `T& selectNth(k)`, `selectNth(k, less)`, `selectNthBy(k, key)` | introselect: quickselect, falling back to median of medians | O(n) | none | no |
| `i64[..>] sortedIndexes()`, `sortedIndexes(less)`, `sortedIndexesBy(key)` | stable merge sort of indexes; the array itself is unchanged | O(n log n) | the result, plus n/2 indexes | yes |
| `bool isSorted()`, `isSorted(less)`, `isSortedBy(key)` | one pass | O(n) | none | |

Scratch space is taken on a scratch data stack and released when the call returns.

- **Which to use**:
  - **`sort`**, unless there is a reason for another. Input that is already sorted, or sorted in reverse, takes one pass.
  - **`stableSort`** when equal elements must keep their order. Sorting by a second key and then, stably, by the first gives an order by both:
    ```
    people.sortBy((Person p) => p.name);
    people.stableSortBy((Person p) => p.age);      // by age, and by name within each age
    ```
    It is also the fastest on input made of a few sorted runs, such as two sorted arrays appended to each other.
  - **`radixSort`** for large arrays of numbers or strings, or of records with such a key. It compares no elements, so it overtakes `sort` once there are a few thousand. For strings it gains most when they share long prefixes, such as paths or URLs.
  - **`partialSort(k)`** for the top `k`. Afterwards the first `k` elements are the `k` smallest, in order, and the rest are in no particular order. A `k` beyond the length sorts everything.
    ```
    counts.partialSort(10, (Count a, Count b) => a.n > b.n);    // the ten most common, first
    ```
  - **`selectNth(k)`** for a median or a percentile without a full sort. It puts at `k` the element a sort would put there, with nothing larger before it and nothing smaller after it, and returns a reference to it. A `k` outside the array is a bug.
    ```
    f64 median = times.selectNth(times.len / 2);
    f64 p99 = times.selectNth(times.len * 99 / 100);
    ```
  - **`sortedIndexes`** when the elements shouldn't move: a read-only slice; records so large that moving them costs more than sorting indexes; or several orders of one array at once. It is the one sort that works on read-only slices.
    ```
    i64[..>] byKey = rows.sortedIndexesBy((Row r) => r.key);
    for (i64 i : byKey) { println(rows[i].line); }
    ```
- **Radix keys are numbers or strings, and the key's type picks the mode**, through overloading (DESIGN §6.5):
  ```
  lines.radixSort();                              // string[..>]: MSD
  ids.radixSort();                                // u64[..>]: LSD
  rows.radixSortBy((Row r) => r.key);             // a string key: MSD
  rows.radixSortBy((Row r) => r.timestamp);       // an i64 key: LSD
  ```
  - **Numbers** (`K implements number`) go least significant byte first: one pass over every element per byte of the key. A pass is skipped when every key has the same byte there, so small values in a wide type cost little. Floats sort by value with NaN last, as in the other sorts (DESIGN §10.1).
  - **Strings** go most significant byte first. The first byte splits them into buckets, and only a bucket with more than one string is split again, by the next byte, so each string is read only as far as it takes to tell it apart. A string that ends sorts before the longer strings it is a prefix of, which gives `sort()`'s byte order. Small buckets are finished by insertion sort, and the buckets still to split are kept in a work list rather than by recursion.
  - **Both are stable.** Keys of other types use `sort`.
  - **One name covers both**, since they give the same result and differ only in speed. An in-place MSD sort, which needs no scratch space but isn't stable, is left out: `sort` is already the in-place choice.
- **Text**: `sort()` orders strings byte by byte. For file names and version numbers, `compareNatural` (§3.3) puts `file2` before `file10`:
  ```
  files.sort((string a, string b) => a.compareNatural(b) < 0);
  ```
- **A comparator should be a strict order**: `<`, not `<=`. One that isn't, even one that answers at random, gives some order, but never reads outside the array and always finishes.
- **`By` forms call `key` about n log n times**, so a key should be cheap: a field, or arithmetic on fields.

**In place.**

| Method | Notes |
|---|---|
| `void reverse()` | |
| `void shuffle(Rng& r)` | Fisher–Yates (§8.2) |
| `void swap(i64 i, i64 j)` | |
| `void fill(T x)` | |

- **A reference keeps pointing at its slot.** After a sort, a `T&` to `a[0]` sees whatever is now first. Nothing moves or is freed, so this is a question of logic, not of safety, and it isn't a removal under rule R.
- **On a cursor array, the sorts, `reverse` and `shuffle` permute the index, not the data.** Elements stay where they are, and a reference follows its element, unlike on a plain array (DESIGN §4.3).

**Searching.**

| Method | Fails with | Notes |
|---|---|---|
| `i64 indexOf(T x)` | `MISSING` | compares with `==` |
| `bool contains(T x)` | | |
| `i64 findIndex(func(T => bool) pred)` | `MISSING` | |
| `bool any(pred)`, `bool all(pred)`, `i64 count(pred)` | | |
| `i64 binarySearch(T x)` | `MISSING` | the array must be sorted ascending; the index of an equal element |
| `i64 lowerBound(T x)`, `i64 upperBound(T x)` | | the first index `>= x`, or `> x`; never fail |

`binarySearch`, `lowerBound` and `upperBound` each have a `By` form that takes a key function, such as `lowerBoundBy(k, key)`, with the key function last, as in `partialSortBy(k, key)`.

**New values.** Results are built in place (DESIGN §3.3).

| Method | Fails with | Notes |
|---|---|---|
| `T[..>] filter(func(T => bool) keep)` | | `var adults = dogs.filter((Dog d) => d.age >= 18);` |
| `U[..>] map(func(T => U) f)` | | |
| `A fold(A init, func(A, T => A) f)` | | |
| `T sum()` | | numbers; overflow stops the program |
| `T min()`, `T max()` | `EMPTY` | |
| `i64 minIndexBy(key)`, `i64 maxIndexBy(key)` | `EMPTY` | an index, so the caller can take a reference |

**Editing.**

| Method | Needs | Notes |
|---|---|---|
| `void pushAll(T[:] xs)` | insert at the end | `FULL` on a capped array without room |
| `void retain(func(T => bool) keep)` | remove at the end | keeps order; a removal under rule R |
| `void dedup()` | remove at the end | removes adjacent equal elements, so sort first to remove every duplicate |
| `void truncate(i64 n)` | remove at the end | keeps the first `n` |

**Iterators.**

| Method | Notes |
|---|---|
| `ChunkIterator chunks(i64 n)` | slices of `n` elements, the last one shorter |
| `WindowIterator windows(i64 n)` | every run of `n` consecutive elements |

Both yield slices, so on cursor arrays they wait for cursor-array slices (DESIGN §17).

### 7.2 Heaps (v1)
On arrays that can grow and shrink at the end (`T[..~]`):

```
void a.heapPush(T x, func(T, T => bool) less)   // FULL on a capped array without room
T a.heapPop(func(T, T => bool) less)            // EMPTY
void a.heapify(func(T, T => bool) less)
```

```
class Step { i64 dist; i64 node; }

Step[..~] queue = [];
queue.heapPush(new Step(0, start), (Step a, Step b) => a.dist < b.dist);
while (Step s : queue.heapPop((Step a, Step b) => a.dist < b.dist)) {    // until the queue is empty
    ...
}
```

- **Pops come out smallest first** by `less`, and the smallest is always `a[0]`.
- **Pass the same `less` every time.** A different one gives a wrong order, never unsafe memory.

### 7.3 `hashmap` (Decided)
Specified in DESIGN §4.14. The library adds one method:

```
V m.getOr(K key, V fallback)
```

### 7.4 `hashset` (v1)
```
hashset<string> seen = [];
if (seen.add(word)) { unique++; }      // true when it wasn't there yet
```

```
bool s.add(T x)
bool s.has(T x)
void s.remove(T x)      // MISSING
i64 s.len
void s.clear()
```

- **Built in, like `hashmap`**: the same layout without the values, the same key rules (`T` must be `hashable`, DESIGN §10.1), and the same reference rules (DESIGN §4.14, REFERENCES §9). `hashset` becomes a built-in type name.
- **For-each visits elements in insertion order.**

### 7.5 `pool` (v1)
DESIGN §3.5, made concrete:

```
pool<Conn> conns = [];
Handle h = conns.add(new Conn(fd));      // reuses a freed slot, or grows
Conn& c @gone = conns.get(h);            // MISSING after h was freed
@_ = conns.free(h);                      // MISSING when already freed
for (Conn& c : conns) { ... }            // live slots only
```

- **A `Handle` is a slot index and a generation**, 8 bytes in all.
  - Freeing a slot bumps its generation, so a stale handle fails with `MISSING` instead of reading the slot's next value.
  - On failure, `get` still returns a reference to the slot, which is safe because a pool slot always holds a `T` (REFERENCES §5.4).
- **`T&` references into a pool are exempt from rule R** (REFERENCES §5.4), and may be stored in fields (REFERENCES §7.4). A stale one reads the replacement: a logic bug, never memory corruption.
- **Layout**: one grow-only array of slots, each with a 4-byte header that holds its generation and, while the slot is free, the next free slot.
- **Built in** like `hashmap`, so it is created with `[]` and is a built-in type name.

### 7.6 Later containers
- **`sortedmap<K, V>`**: ordered by key, so `K` must be `ordered` (DESIGN §10.1).
  - Methods: `get`, `put`, `remove`, `first()`, `last()`, `floor(k)`, `ceiling(k)`, `range(lo, hi)` as an iterator, and for-each in key order.
  - Layout: entries are appended to an array that only grows, as in `hashmap`, and a B-tree of their indexes lives in a pool. So entries never move, and references survive inserts.
- **`bitset`**: grows to fit the largest index.
  - Methods: `set(i)`, `unset(i)`, `has(i)`, `count()`, `and(other)`, `or(other)`, and for-each over the set bits.

---

## 8. Math, random numbers, hashing and encoding (v1)

### 8.1 `math`
```
// F is f32 or f64: each of these is <F implements float>
F math.sqrt(F x)        cbrt, exp, log, log2, log10
F math.pow(F x, F y)
F math.hypot(F x, F y)
F math.sin(F x)         cos, tan, asin, acos, atan, sinh, cosh, tanh
F math.atan2(F y, F x)
F math.floor(F x)       ceil, trunc, round (half away from zero)
math.PI, math.TAU, math.E

<T implements int> T math.gcd(T a, T b)
<T implements int> T math.lcm(T a, T b)                 // overflow stops the program
<T implements int> T math.ipow(T base, i64 exp)         // overflow stops the program
<T implements int> T math.isqrt(T x)
i64 math.roundToI64(f64 x)          // RANGE for NaN, infinities and values outside i64
```

- **The float functions are generic over `f32` and `f64`**, not overloaded: `<F implements float> F math.sqrt(F x)`.
- **They follow IEEE 754 and never stop the program**: `math.sqrt(-1.0)` is NaN.

Bit operations, as methods on every integer type:
```
i64 x.popCount()    i64 x.leadingZeros()    i64 x.trailingZeros()
T x.rotateLeft(i64 n)    T x.rotateRight(i64 n)    T x.reverseBytes()
bool x.isPowerOf2()
```

### 8.2 `random`
```
Rng random.seeded(u64 seed)          // the same sequence on every run
Rng random.fromOs()                  // seeded from getrandom
u64 r.nextU64()
f64 r.nextF64()                      // in [0, 1)
<T implements int> T r.range(T lo, T hi)    // in [lo, hi), unbiased; lo >= hi is a bug
bool r.chance(f64 p)

void random.secureBytes(u8[..>]& out, i64 n)   // from the kernel: keys, tokens, salts
u64 random.secureU64()
```

- **`Rng` is xoshiro256\*\***: 32 bytes, fast, with good statistics. It is **not for secrets**; `random.secure*` is.
- **`getrandom` never fails** on a running Linux system; it can only block briefly at early boot.

### 8.3 `hash`
```
u64 hash.fnv1a(string bytes)
u64 hash.xxh3(string bytes, u64 seed)
u32 hash.crc32(string bytes)              // the IEEE polynomial, as gzip and zip use
u8[.32] hash.sha256(string bytes)
```

Incremental forms, for data that arrives in pieces:
```
Sha256 h = new Sha256();
h.update(chunk);
u8[.32] digest = h.finish();
println("${digest.hex()}");
```
`Crc32` works the same way.

- **`hashmap` doesn't use these.** It uses its own keyed hash with a per-run seed (DESIGN §4.14).
- **Later**: SHA-512, BLAKE3, HMAC, and SHA-1 and MD5 for checking old checksums.

### 8.4 `hex` and `base64`
```
u8[.] bytes.hex()                                  // on any u8 array or string: two lowercase digits per byte
void hex.decode(string text, u8[..>]& out)         // INVALID: an odd length or a non-hex digit
u8[.] base64.encode(string bytes)                  // RFC 4648, with padding
void base64.decode(string text, u8[..>]& out)      // INVALID
u8[.] base64.encodeUrl(string bytes)               // the URL-safe alphabet, without padding
void base64.decodeUrl(string text, u8[..>]& out)   // INVALID
```

- **`hex()` depends on the receiver.** On an integer it formats the value (DESIGN §4.13). On a `u8` array it encodes the bytes, so `"hi".hex()` is `"6869"`.

---

## 9. Data formats

### 9.1 CSV: `zink.csv` (v1)
```
CsvReader r = new CsvReader(text);
r.separator = ';';                         // ',' by default
u8[..>] strings = [];                      // fields that had "" inside quotes, decoded
string[..~] fields = [];                   // cleared by every call, so it must allow removal
while (true) {
    u8 st = @r.next(fields, strings);
    if (st == status.END) { break; }
    if (st) {
        ParseError e = r.error();
        errprintln("line ${e.line}: ${e.message}");
        return st;
    }
    println(fields[0]);
}
```

- **`r.next(string[..~]& fields, u8[..>]& strings)` clears `fields`**, then appends one view per field. Clearing is a removal, so `fields` must allow removal at the end (`[..~]`).
- **A field is a view into the input whenever it can be**: an unquoted field, or a quoted one with no `""` inside (the view leaves the quotes out).
- **Only fields with doubled quotes are decoded**, into `strings`. `next` never clears `strings`; clear it yourself once no earlier field is kept.
- **The format is RFC 4180's**:
  - quoted fields may contain the separator and newlines;
  - lines end in `\n` or `\r\n`;
  - a UTF-8 byte-order mark at the start is skipped.
- **It fails with `INVALID`** for a quote that is never closed, or for text after a closing quote (`"a"b`). With `r.strict = true`, every row must also have as many fields as the first.
- **Fields**: `string input; u8 separator = ','; u8 quote = '"'; bool strict = false;`.
- **`csvsort.zn` can use it** to handle quoted fields, which it currently doesn't.

`CsvWriter`:
```
u8[..>] out = [];
CsvWriter w = new CsvWriter(out);
w.writeRow(["name", "note"]);
w.writeField(name);
w.writeField(note);
w.endRow();
u8 err = @File.writeAll("out.csv", out);
```

- **A field is quoted only when it needs to be**: when it contains the separator, a quote, `\r` or `\n`, or starts or ends with a space.
- **Rows end in `\n`.** `w.crlf = true` gives RFC 4180's `\r\n`.
- **`w.separator = '\t'`** writes tab-separated values.
- **It sets no status** (§1.3, rule 7).

### 9.2 Parsed trees: `zink.data` (later)
**All four formats parse into the same tree**: JSON, JSON5, TOML and YAML.
- Code that reads configuration doesn't care which format the file was in.
- Converting between formats is a parse and a write (§9.8).

```
import zink.data.*;
import zink.toml.*;

u8[..>] text = [];
u8 err = @File.readAll("app.toml", text);
if (err) { ... }

u8[..>] strings = [];                    // strings that had escapes, decoded
Node[..>] tree = [];
TomlParser p = new TomlParser(text);
Value cfg @bad = p.parse(tree, strings);
if (bad) {
    ParseError e = p.error();
    errprintln("app.toml:${e.line}:${e.column}: ${e.message}");
    return bad;
}

string host @_ = cfg.get("server").get("host").asStringOr("localhost");
i64 port @badPort = cfg.get("server").get("port").asI64();    // MISSING, TYPE, RANGE
for (Value user : cfg.get("users").items()) {
    string name @_ = user.get("name").asStringOr("?");
    println(name);
}
```

**Memory**: three buffers, all the caller's.
- **`text`**, the input. Strings without escapes, and every number, are views into it.
- **`strings`**, where strings with escapes are decoded. Most keys and values have none, so it usually stays small.
- **`tree`**, an array of fixed-size `Node`s, one per key or value, in document order.
- **Views stay valid** because all three only grow while the tree is in use. Clearing all three reuses them for the next document (JSON Lines, §9.3).

**Navigation never fails.**
- A missing key or index gives an absent value, and so does `get` on something that isn't an object.
- Only reading a value sets a status.
- So a chain such as `cfg.get("server").get("port").asI64()` needs one check, at the end, and drops no status along the way.

`Value` is a view of one node, `{ Node[:] tree; i64 at; }`, 24 bytes. It is rooted at the tree, and through the tree at `text` and `strings`.

| Method | Result | Fails with |
|---|---|---|
| `Kind kind()` | `NULL`, `BOOL`, `INT`, `FLOAT`, `STRING`, `DATETIME`, `ARRAY`, `OBJECT`, or `ABSENT` | |
| `bool exists()` | false for an absent value | |
| `bool isNull()` | | |
| `Value get(string key)` | an object's value for `key`; the last one when the key repeats | |
| `Value at(i64 i)` | an array's element `i`; O(i), so iterate with `items()` | |
| `Value pointer(string p)` | an RFC 6901 path: `"/servers/0/port"` | |
| `i64 len()` | elements or entries; 0 for anything else | |
| `ValueIterator items()` | an array's elements | |
| `EntryIterator entries()` | an object's entries in document order, as `Entry { string key; Value value; }` | |
| `string asString()` | | `MISSING`, `TYPE` |
| `i64 asI64()`, `u64 asU64()` | | `MISSING`, `TYPE`, `RANGE` |
| `f64 asF64()` | an integer converts | `MISSING`, `TYPE` |
| `bool asBool()` | | `MISSING`, `TYPE` |
| `DateTime asDateTime()` | TOML's dates and times (§5.3) | `MISSING`, `TYPE` |
| `asStringOr(fallback)`, `asI64Or`, `asF64Or`, `asBoolOr` | the fallback when the value is absent or null | `TYPE`, with the fallback |
| `string text()` | a scalar as written: a number's digits, even for one too big for `i64` | `MISSING`, `TYPE` |
| `i64 line()` | the line it starts on in the input; 0 for a built tree | |
| `string tag()` | its YAML tag, or `""` | |

- **`TYPE` is never a silent conversion.** The string `"8080"` isn't an `i64`, and `asI64()` on `3.0` fails.
- **`get` scans the object's entries**, which is fast for the small objects that configuration files have. For a big object read many times, build a `hashmap<string, Value>` from `entries()` once.

**`Node`**, for code that walks the tree itself (32 bytes):
```
class Node {
    Kind kind;        // u8
    u8 flags;         // how a number or string was written: hex, escaped, quoted, ...
    u16 column;       // where it starts, clamped at 65535
    u32 size;         // nodes in this subtree, itself included: the next sibling is at index + size
    u32 count;        // an array's elements or an object's entries; an alias's target index
    u32 line;
    string text;      // a key, a string, or a number's or date's text as written
}
```

- **An object's children are entries**, each a `KEY` node followed by its value's subtree.
- **Some nodes are wrappers**: YAML's `ALIAS`, `ANCHOR` and `TAG`. `Value` passes through them.

**`ParseError`**:
```
class ParseError { u8 code; i64 line; i64 column; string message; }
```

- **`code` is the status the parse failed with**, or 0 if it didn't fail.
- **`line` and `column` count from 1**, and the column counts bytes.
- **`message` is static text**, such as `"expected ',' or '}'"`.

**`Parser`**, for choosing a format at run time, such as by file extension:
```
interface Parser {
    mut Value parse(Node[..>]& tree, u8[..>]& strings);
    ParseError error();
}
```

**Limits and input**:
- **Nesting deeper than `maxDepth` fails with `FULL`.** Every parser has the field, 1024 by default.
- **The parsers don't recurse**, so a deeply nested file can't overflow the native stack.
- **Input must be UTF-8.** Invalid UTF-8 is `INVALID`.

**Reading into classes** is open (§11): `cfg.into<Config>()`, filling fields by name, and writing a class out the same way. Both need compile-time access to a class's fields, which Zinkite doesn't have (there is no reflection, DESIGN §1).

### 9.3 JSON: `zink.json` (later)
The format is RFC 8259.

```
JsonParser p = new JsonParser(text);   // fields: string input; bool json5 = false; i64 maxDepth = 1024
Value v @bad = p.parse(tree, strings);
```

- **Numbers keep their text.** `asI64` and `asF64` convert on demand, so a number beyond `i64` isn't lost: `text()` returns its digits.
- **Duplicate keys are kept**, and `get` returns the last one, as JavaScript and Python do. `entries()` shows them all.
- **`\u` escapes are decoded to UTF-8.** A lone surrogate (`"\ud800"`) becomes U+FFFD, as `utf8()` does (DESIGN §4.13).
- **A byte-order mark is skipped.** Anything other than whitespace after the value is `INVALID`.

**Streaming: `JsonReader`**, for input too big for a tree, or for reading straight into a program's own classes:

```
JsonReader r = new JsonReader(text);
while (JsonEvent e : r.next()) {
    switch (e.kind) {
        case KEY => { if (e.text != "id") { r.skipValue(); } }
        case NUMBER => { i64 id @_ = e.asI64(); ... }
        default => {}
    }
}
if (r.error().code) { ... }
```

- **`JsonEvent`** is `{ JsonToken kind; string text; i64 line; }`. The kinds are `BEGIN_OBJECT`, `END_OBJECT`, `BEGIN_ARRAY`, `END_ARRAY`, `KEY`, `STRING`, `NUMBER`, `TRUE`, `FALSE` and `NULL`.
- **`e.text` is the raw text**, without the quotes but with any escapes. That's enough for comparing keys, which rarely have escapes.
  - `string e.decode(u8[..>]& strings)` gives the decoded text.
  - It is a view into the input when there are no escapes, and is decoded into `strings` otherwise.
- **`e.asI64()` and `e.asF64()` read a `NUMBER`**, failing as the tree's accessors do (§9.2).
- **`r.skipValue()` skips the value after a `KEY`**, a whole object or array included.
- **The loop stops at the end of the input, or at an error.** Afterwards, `r.error().code` is 0 if the input was complete.
- **`r.skipValue()` acts on the reader the loop calls**, since the repeat loop names it (DESIGN §9.4).

**JSON Lines** (`.jsonl`), one value per line:
```
for (string line : text.lines()) {
    tree.clear();
    strings.clear();
    JsonParser p = new JsonParser(line);
    Value v @bad = p.parse(tree, strings);
    ...
}
```

Writing JSON is in §9.7.

### 9.4 JSON5 (later)
JSON5 is JSON plus what people write in hand-edited configuration files. `JsonParser` reads it when `p.json5 = true`, which a `.json5` file usually wants.

What it adds to JSON:
- `//` and `/* */` comments;
- trailing commas in arrays and objects;
- unquoted keys that are identifiers: `{ port: 8080 }`;
- single-quoted strings, and strings continued onto the next line with a `\` at the end of the line;
- numbers: hexadecimal (`0x1F`), a leading or trailing decimal point (`.5`, `5.`), a leading `+`, `Infinity`, `-Infinity` and `NaN`;
- a few more whitespace characters, such as U+00A0.

**It parses into the same tree.**
- `Infinity` is a `FLOAT` that `asF64` reads.
- JSON has no form for it, so `JsonWriter` writes `null` in its place and `finish()` reports `TYPE`, unless the writer's `json5` is set.
- **JSON is valid JSON5**, so writing JSON already writes JSON5. With `w.json5 = true`, the writer also leaves keys unquoted where they are identifiers, and writes `Infinity` and `NaN`.

### 9.5 TOML: `zink.toml` (later)
The format is TOML 1.0.0.

```
TomlParser p = new TomlParser(text);
Value cfg @bad = p.parse(tree, strings);
```

| TOML | Tree |
|---|---|
| `[table]`, dotted keys (`a.b = 1`), inline tables (`{ x = 1 }`) | `OBJECT` |
| `[[array.of.tables]]` | `ARRAY` of `OBJECT` |
| arrays | `ARRAY` |
| basic, literal and multi-line strings | `STRING` |
| integers: decimal, `0x`, `0o`, `0b`, with `_` | `INT` |
| floats, `inf`, `nan` | `FLOAT` |
| booleans | `BOOL` |
| offset date-times, local date-times, local dates, local times | `DATETIME`; `asDateTime().parts` says which |

- **A table split across the file is joined.** In `[a]` … `[b]` … `[a.c]`, `c` ends up inside `a`.
  - Each table lists its keys in the order they first appear.
  - The parser groups them in a scratch array of its own, which is released when `parse` returns.
- **Breaking TOML's own rules is `INVALID`**, with the line:
  - a key defined twice;
  - a table defined twice;
  - adding to an inline table after it is closed;
  - a value with no key.
- **TOML has no null**, so a TOML tree never has `NULL` nodes.

### 9.6 YAML: `zink.yaml` (later)
The format is YAML 1.2.2, with the core schema.

```
YamlParser p = new YamlParser(text);
Value doc @bad = p.parse(tree, strings);         // exactly one document
```

**Supported**:
- block mappings and sequences, and flow `{ }` and `[ ]`;
- plain, single-quoted and double-quoted scalars;
- literal `|` and folded `>` block scalars, with chomping (`|-`, `|+`) and indentation indicators;
- comments, and the `%YAML` and `%TAG` directives;
- **several documents per file** (`---`, `...`). `p.parseNext(tree, strings)` returns each document in turn and fails with `END` after the last, so a list of Kubernetes manifests reads in a loop;
- **anchors `&name` and aliases `*name`.** A `Value` of an alias reads as the node it refers to, without copying it;
- **tags**:
  - the core ones (`!!str`, `!!int`, `!!float`, `!!bool`, `!!null`, `!!seq`, `!!map`) change how a scalar is read, as in `!!str 1.0`;
  - any other tag is kept, and `v.tag()` returns it;
- **merge keys** (`<<: *defaults`). They come from YAML 1.1, but are common in CI and Compose files.
  - `get` falls back to the merged mappings when a key isn't found directly.
  - `p.mergeKeys = false` turns this off.

**Plain scalars are typed by the core schema**:

| Written | Kind |
|---|---|
| `null`, `Null`, `NULL`, `~`, or nothing | `NULL` |
| `true`, `True`, `TRUE`, `false`, `False`, `FALSE` | `BOOL` |
| `123`, `-12`, `0o17`, `0x1F` | `INT` |
| `1.5`, `1e3`, `.inf`, `-.inf`, `.nan` | `FLOAT` |
| anything else, including `yes`, `no`, `on`, `off` and `1.0.3` | `STRING` |

- **So `country: NO` is the string `"NO"`, not false.** YAML 1.1's extra booleans, the source of the "Norway problem", aren't recognized.
- **Quoted scalars are always strings.**

**Left out**:
- **Keys that are collections** (`? [a, b] : value`) fail with `UNSUPPORTED`.
- **Keys that are numbers or booleans** are kept as their text, so `200:` in an OpenAPI file is the key `"200"`.

**Errors** are `INVALID`, with the line and column: bad indentation, a tab in indentation, an undefined alias, or a duplicate key.

**Safety**:
- **Parsing never runs code**, and never builds program objects from tags, unlike `yaml.load` in some libraries. A tag is only text.
- **Aliases aren't expanded in the tree**, so a "billion laughs" file stays small.
  - Writing it out as JSON or TOML expands them.
  - Writers stop expanding after `maxExpansion` nodes (1,000,000 by default) and report `FULL` at `finish()`.

**Comments aren't kept in the tree**, so a parse followed by a write loses them. Editing a YAML or TOML file while keeping its comments and layout needs a different, lossless tree, and is out of scope.

### 9.7 Writing (later)
**`DataWriter`** is the interface every writer implements:

```
interface DataWriter {
    mut void beginObject();
    mut void beginArray();
    mut void end();                    // closes the innermost object or array
    mut void key(string k);
    mut void write(string s);
    mut void write(i64 n);
    mut void write(u64 n);
    mut void write(f64 x);
    mut void write(bool b);
    mut void write(DateTime d);
    mut void write(Value v);           // a whole parsed subtree
    mut void writeNull();
    mut void finish();                 // TYPE, FULL
}
```

- **`write` is overloaded** (DESIGN §6.5): `w.write(80)` writes an `i64`, a `u32` goes to `write(u64)`, and an `f32` to `write(f64)`.
- **Only `finish()` sets a status** (§1.3, rule 7).
- **Misuse is a bug and stops the program**: a value inside an object without a key, `end()` with nothing open, or `finish()` with something still open.
- **`finish()` fails with**:
  - `TYPE` when a value has no form in the output format, such as a NaN in JSON or a null in TOML. JSON gets `null` in its place, and TOML leaves the key out.
  - `FULL` when alias expansion hit its limit.
- **The output is complete and well formed even then.**

**`JsonWriter`** streams JSON into a buffer:
```
u8[..>] out = [];
JsonWriter w = new JsonWriter(out);
w.indent = 2;                          // 0, the default, writes everything on one line
w.beginObject();
w.key("name");
w.write("zink");
w.key("ports");
w.beginArray();
w.write(80);
w.write(443);
w.end();
w.end();
u8 err = @w.finish();
println(out);
```

- **Fields**: `i64 indent = 0; bool json5 = false; bool asciiOnly = false; i64 maxExpansion = 1000000;`.
- **Strings are escaped as JSON requires**: `"`, `\` and control characters. `asciiOnly` escapes everything beyond ASCII too.
- **Floats are written in the shortest form that reads back the same** (`0.1`).
- **`json.write(u8[..>]& out, Value v, i64 indent)`** writes a whole tree in one call (`TYPE`, `FULL`).

**TOML and YAML are written from a tree**, since TOML must group each table's keys and YAML picks its styles from what a value contains:
```
void toml.write(u8[..>]& out, Value root)    // TYPE: a root that isn't an object, or a null; FULL
void yaml.write(u8[..>]& out, Value root)    // FULL
```

- **`toml.write`**:
  - writes nested objects as `[a.b]` tables;
  - writes arrays whose elements are all objects as `[[a]]`;
  - writes other objects inside arrays as inline tables.
- **`yaml.write`**:
  - uses block style;
  - quotes a string whenever it would read back as something else: `"yes"`, `"no"`, `"1.0"`, `"null"`, `""`. That includes YAML 1.1's booleans, for older readers;
  - uses `|` for multi-line strings;
  - writes anchors and aliases back as `&` and `*`.

**`TreeBuilder`** builds a tree from program data: for writing TOML or YAML, or for code that takes a `Value`.
```
u8[..>] strings = [];                  // the text of the numbers the program writes
Node[..>] tree = [];                   // its nodes view strings, so strings comes first (rule S)
TreeBuilder b = new TreeBuilder(tree, strings);
b.beginObject();
b.key("server");
b.beginObject();
b.key("host");
b.write("localhost");
b.key("port");
b.write(8080);
b.end();
b.end();
u8 built = @b.finish();

u8[..>] out = [];
u8 err = @toml.write(out, b.root());
```

- **`TreeBuilder` implements `DataWriter`.** So a function written once can either write JSON directly or build a tree for TOML and YAML:
  ```
  <W implements DataWriter> void writeConfig(W& w, Config c) { ... }
  ```
- **Strings passed to `write` are stored as views, not copied**, so they must outlive the tree, which rule S checks.
- **Numbers are formatted into `strings`.**

### 9.8 Example: YAML to JSON
```
#!/usr/bin/env zink
// yaml2json: read one YAML document on stdin, write it as JSON on stdout.
import zink.data.*;
import zink.json.*;
import zink.yaml.*;

u8 main() {
    u8[..>] text = [];
    u8 err = @stdin.readAll(text);
    if (err) {
        errprintln("yaml2json: ${status.message(err)}");
        return err;
    }

    u8[..>] strings = [];
    Node[..>] tree = [];
    YamlParser p = new YamlParser(text);
    Value doc @bad = p.parse(tree, strings);
    if (bad) {
        ParseError e = p.error();
        errprintln("yaml2json: line ${e.line}, column ${e.column}: ${e.message}");
        return bad;
    }

    u8[..>] out = [];
    @err = json.write(out, doc, 2);
    if (err == status.TYPE) {
        errprintln("yaml2json: .inf and .nan have no JSON form; wrote null");
    }
    println(out);
    return 0;
}
```

Swapping `YamlParser` for `TomlParser`, or `json.write` for `yaml.write`, gives the other converters.

### 9.9 Glob patterns and regular expressions (later)
**`glob`**:
```
bool glob.match(string pattern, string name)          // INVALID for a malformed pattern, such as "[a-"
GlobIterator fs.glob(string pattern, u8[..~]& path)   // "src/**/*.zn"
```

- **Patterns**:
  - `*` matches anything except `/`, and `?` matches one character;
  - `[abc]`, `[a-z]` and `[!a-z]` match one character from a class;
  - `**` matches any number of directories.
- **Matching never backtracks exponentially.**
- **`fs.glob` is built on `fs.walk`** (§4.3), so its paths follow the same rules. It only descends into directories the pattern can match.

**`Regex`**:
```
Regex re @bad = Regex.compile("([a-z]+)=([0-9]+)");   // INVALID
bool re.matches(string s)                  // the whole of s
Match re.find(string s)                    // the first match; MISSING
MatchIterator re.findAll(string s)
string m.group(i64 i)                      // "" for a group that didn't take part
u8[.] re.replaceAll(string s, string with) // $1 in `with` refers to group 1
```

- **Matching is linear in the input** (a Pike VM, as in RE2 and Rust's `regex`), so no pattern can take exponential time. The price is no backreferences and no lookaround.
- **Syntax**:
  - character classes, plus `\d`, `\w` and `\s`;
  - anchors;
  - lazy quantifiers (`*?`, `+?`) and counted ones (`{m,n}`);
  - named groups (`(?<name>...)`);
  - flags such as `(?i)`.
- **Classes beyond ASCII come later**, with the Unicode tables.
- **A `Regex` is variable-size**, since it holds its compiled program inline. Keep it in a local or a field, not in an array that needs indexing (REFERENCES §6, V3).
- **A `Match` holds up to 16 groups.** A pattern with more fails to compile with `INVALID`.

---

## 10. Later

### 10.1 Networking: `zink.net`
- **TCP**:
  - `TcpStream.connect(string host, u16 port)` returns a handle like `File`, with `read`, `write` and `close`;
  - `TcpListener.listen(string address, u16 port)` and `accept()` serve connections;
  - then UDP.
- **Name lookup**: an own resolver that reads `/etc/hosts` and `/etc/resolv.conf`, since static binaries can't use glibc's NSS.
- **I/O style**: blocking calls with timeouts first, and an event loop (`poll` over several handles) after that. Programs stay single-threaded.
- **HTTP**: an HTTP/1.1 client, `http.get(url, out)` and `http.post(url, body, out)`. HTTPS needs TLS, the largest piece. Whether it is a C library inside the runtime (DESIGN §12.2) or Zinkite's own is open.
- **Status names**:

  | Name | Number | `errno` |
  |---|---|---|
  | `ADDRESS_IN_USE` | 98 | `EADDRINUSE` |
  | `CONNECTION_RESET` | 104 | `ECONNRESET` |
  | `CONNECTION_REFUSED` | 111 | `ECONNREFUSED` |
  | `HOST_UNREACHABLE` | 113 | `EHOSTUNREACH` |

### 10.2 Compression: `zink.compress`
- `gzip.decompress(string data, u8[..>]& out)` (`INVALID`) and `gzip.compress(string data, u8[..>]& out, i64 level)`.
- Raw deflate and zlib framing.
- Streaming forms for large files.
- zstd after that.

### 10.3 Threads: `zink.thread`
Follows Goose:
- each worker is compiled as a separate program, with its own data stacks;
- workers exchange flat values, with no references, through typed queues.

So no reference ever crosses a thread (REFERENCES §12).

### 10.4 Big integers: `zink.bigint`
- **`BigInt` is variable-size**, with its digits inline, so it follows rule V. Each result is a new value, built in place: `BigInt c = a.plus(b);`.
- **It has parsing and formatting**, and reads JSON numbers beyond `i64` through `Value.text()`.

### 10.5 Tests: `zink.test`
```
// test/SplitTest.zn
class SplitTest {
    static void testKeepsEmptyParts() {
        string[..>] parts = "a,,b".split(",");
        assertEq(parts.len, 3);
        assertEq(parts[1], "");
    }
}
```

- **`zink test` runs every `static void test...()` method** in files whose names end in `Test.zn`.
- **`assert(cond)`, `assertEq(a, b)` and `assertNe(a, b)`**. On failure, `assertEq` prints both values.
- **Each test runs in its own child process.** A failed assertion stops the program like a bug (DESIGN §7.6), so this way it ends only that one test, and every test starts with fresh data stacks.
- **Whether `assert` is a statement** (`GRAMMAR.md` §8) or these functions is open.

### 10.6 Graphics: `vendor.glfw`, `vendor.sdl3`, `vendor.gl`, `vendor.vulkan`
Bindings to C libraries for windows, input and the GPU. They aren't part of the standard library. Each wraps a third-party library with its own releases, so they ship as separate packages under `vendor.*` (DESIGN §8.4). They are designed here because the language features they need (DESIGN §17) should be designed with them.

| Package | Wraps | Gives |
|---|---|---|
| `vendor.glfw` | GLFW 3.4 | windows, OpenGL contexts, Vulkan surfaces, keyboard, mouse, gamepads, time |
| `vendor.sdl3` | SDL 3.2 or later | windows, Vulkan surfaces, input, gamepads, audio, timers |
| `vendor.gl` | OpenGL 3.3 core to 4.6 | the OpenGL API |
| `vendor.vulkan` | Vulkan 1.3 and later | the Vulkan API and its extensions |

Three pairings are supported:

| Windows and input | Graphics | When to choose it |
|---|---|---|
| GLFW | OpenGL | the shortest way to a triangle: shaders are GLSL text that the driver compiles |
| GLFW | Vulkan | full control of the GPU, with only a window and input around it |
| SDL3 | Vulkan | the same, plus SDL's audio, gamepads and the rest |

**GLFW and OpenGL**:
```
import vendor.glfw.*;
import vendor.gl.*;

class Vertex { f32 x; f32 y; f32 r; f32 g; f32 b; }      // needs C layout (DESIGN §17)

u8 main() {
    u8 err = @glfw.init();
    if (err) { errprintln("no display: ${status.message(err)}"); return err; }
    defer glfw.terminate();

    glfw.windowHint(glfw.CONTEXT_VERSION_MAJOR, 3);
    glfw.windowHint(glfw.CONTEXT_VERSION_MINOR, 3);
    glfw.windowHint(glfw.OPENGL_PROFILE, glfw.OPENGL_CORE_PROFILE);
    GlfwWindow w @noWindow = glfw.createWindow(800, 600, "triangle");
    if (noWindow) { return noWindow; }
    defer w.destroy();
    w.makeContextCurrent();
    glfw.loadGl();                                         // OpenGL's functions, through glfwGetProcAddress

    Vertex[.3] tri = [new(0.0, 0.5, 1.0, 0.0, 0.0), new(-0.5, -0.5, 0.0, 1.0, 0.0), new(0.5, -0.5, 0.0, 0.0, 1.0)];
    GlBuffer vbo = gl.genBuffer();
    defer gl.deleteBuffer(vbo);
    gl.bindBuffer(gl.ARRAY_BUFFER, vbo);
    gl.bufferData(gl.ARRAY_BUFFER, tri, gl.STATIC_DRAW);  // the array's bytes, as they are
    ...                                                    // vertex layout, shaders, program

    while (!w.shouldClose()) {
        while (GlfwEvent e : w.nextEvent()) {
            if (e.kind == GlfwEventKind.KEY && e.key == glfw.KEY_ESCAPE) { w.setShouldClose(true); }
        }
        gl.clear(gl.COLOR_BUFFER_BIT);
        gl.drawArrays(gl.TRIANGLES, 0, 3);
        w.swapBuffers();
    }
    return 0;
}
```

**SDL3 and Vulkan**, the first steps and the frame loop:
```
import vendor.sdl3.*;
import vendor.vulkan.*;

u8 err = @sdl.init(sdl.INIT_VIDEO);
SdlWindow w @noWindow = sdl.createWindow("game", 1280, 720, sdl.WINDOW_VULKAN);
sdl.loadVulkan();                                          // vkGetInstanceProcAddr, through SDL

VkInstanceCreateInfo info = new();                         // every generated field has an initializer
info.enabledExtensionNames = sdl.vulkanInstanceExtensions();   // a string[:], not a pointer and a count
VkInstance inst @noVulkan = vk.createInstance(info);
vk.loadInstance(inst);
VkSurfaceKHR surface @noSurface = w.createVulkanSurface(inst);
...                                                        // device, swapchain, pipeline

while (running) {
    while (SdlEvent e : sdl.pollEvent()) {
        if (e.kind == SdlEventKind.QUIT) { running = false; }
    }
    u32 image @acquire = vk.acquireNextImageKHR(device, swapchain, u64.MAX, imageReady);
    if (acquire == status.OUT_OF_DATE) { recreateSwapchain(); continue; }
    ...                                                    // record, submit, present
}
```

- **Names follow the C API, with the prefix moved to a namespace class**: `glClearColor` is `gl.clearColor`, `GL_TRIANGLES` is `gl.TRIANGLES`, `glfwCreateWindow` is `glfw.createWindow` and `vkCreateInstance` is `vk.createInstance`. Vulkan's structs and handles keep their names (`VkInstanceCreateInfo`, `VkDevice`). So tutorials and the official references translate line by line. Types of the other packages carry their library's name, as `JsonParser` does (§1.2): `GlfwWindow`, `SdlEvent`, `GlBuffer`.
- **The `gl` and `vulkan` bindings are generated** from Khronos's XML registries (`gl.xml`, `vk.xml`), as the C loaders glad and volk are. `glfw` and `sdl3` are generated from their headers. Each package also has a small C layer for what the generator can't do: event queues and building C structs.
- **No raw pointer reaches Zinkite code**, so the reference rules (`REFERENCES.md`) hold across the boundary:
  - a pointer and a count become a slice, such as `enabledExtensionNames`;
  - text C needs as a NUL-terminated string is copied, with the NUL, onto a scratch data stack for the call;
  - output arrays, such as the result of `vkEnumeratePhysicalDevices`, are appended to an array the caller passes (§1.3, rule 1);
  - Vulkan's `pNext` chains become optional fields of the create-info classes, and the C layer builds the real structs;
  - mapped GPU memory is written by copying (`mapped.write(offset, data)`), not through a view, since its life ends at `vkUnmapMemory`, outside the reference rules.
- **Every field of a generated Vulkan class has an initializer**, so `new()` builds the zeroed struct Vulkan code starts from, with `sType` already set (DESIGN §6.2).
- **Functions are loaded at run time**, once a context or instance exists: `glfw.loadGl()`, `glfw.loadVulkan()` or `sdl.loadVulkan()`, and then `vk.loadInstance(inst)` and `vk.loadDevice(dev)`. Calling a function the driver doesn't provide is a bug: it stops the program and names the function.
- **Events are drained with a repeat loop** (DESIGN §9.4). SDL3 already works that way, with `SDL_PollEvent`. GLFW reports input through C callbacks instead, so its C layer queues the events. `w.nextEvent()` calls `glfwPollEvents` when the queue is empty, and fails with `END` when nothing more has happened.
- **Handles are values.** `GlBuffer`, `GlTexture`, `VkDevice` and the rest are deleted explicitly, usually with `defer`: `defer gl.deleteBuffer(vbo);`, which follows the C name `glDeleteBuffers` (DESIGN §6.4). `delete` is reserved, so there is no `vbo.delete()`.
- **Uploads take slices**: `gl.bufferData(target, data, usage)` and the Vulkan copy functions send a slice's bytes as they are. Classes stored inline without headers already have the layout a GPU buffer expects, once their field order and padding are fixed to C's (DESIGN §17).
- **Statuses**:
  - each Vulkan call that returns a `VkResult` sets a status: 0 for success, `OUT_OF_DATE` for `VK_ERROR_OUT_OF_DATE_KHR` (recreate the swapchain), `NO_MEMORY` when host or device memory runs out, and `GPU` for anything else. `vk.lastResult()` gives the exact `VkResult`, `VK_SUBOPTIMAL_KHR` included;
  - OpenGL reports errors on request, not per call: `@gl.checkError()` fails with `GPU` and `gl.lastError()` gives the `GLenum`. With OpenGL 4.3 or `KHR_debug`, `gl.debugToStderr()` prints the driver's messages as they happen;
  - Vulkan's validation layers are enabled by naming them in `VkInstanceCreateInfo`, as in C, and their messages go to stderr.
- **Shaders**: OpenGL compiles GLSL text at run time, and `gl.getShaderInfoLog` gives the compiler's messages. Vulkan takes SPIR-V, compiled ahead of time from GLSL or HLSL with `glslc` or `glslangValidator` and read with `File.readAll`. Embedding the `.spv` file into the binary at compile time is open (DESIGN §17).
- **One thread**: OpenGL contexts, GLFW windows and SDL windows belong to the main thread, which is where a single-threaded Zinkite program runs anyway.
- **Per-frame memory is free.** What a frame allocates lives on data stacks and is released when the frame's block ends. There is no garbage collector to pause a frame, and no frame arena to write by hand.
- **Platforms**: Linux first, on X11 and Wayland through GLFW 3.4 and SDL3, then Windows. On macOS, OpenGL stops at 4.1, and Vulkan runs through MoltenVK.

---

## 11. Open questions

**Scope**
- **Which tier the data formats are in.** JSON, JSON5, TOML and YAML are "later" here, because CSV is the only format v1's examples need. Configuration files may make TOML or JSON worth moving into v1.
- **Unicode tables**: which version, and whether case mapping, properties and display width ship in v1.
- **TLS** for `zink.net`: a C library in the runtime, or Zinkite's own.

**Language features the library leans on**
- **Reading into classes**: filling a class from a tree by field name, and writing one out, need compile-time access to fields. That could be a restricted, compile-time-only reflection, or generated methods like Rust's `derive`.

**Library design**
- **`stdin`, `stdout` and `stderr` as `File` values** (§2.1) rather than a separate stream type.
- **Scratch space for `stableSort` and `radixSort`**: n/2 and n elements on a scratch data stack. In-place versions avoid it, at some cost in speed.
- **The `Args` option limit** of 64, and whether to check option names at compile time when they are literals.
- **Sorted walks**: `fs.walk` returns entries in directory order. Sorting each directory's names needs more buffer space.
