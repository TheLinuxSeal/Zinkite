# 13. The library: files, processes, time and the command line

This chapter is a reference for the parts of the standard library that talk to the system:
the environment and the standard streams in `zink.lang`, files and paths in `zink.io`,
other programs and the terminal in `zink.os`, clocks and dates in `zink.time`, and
command-line arguments and colors in `zink.cli`. The text methods of `zink.lang` are in
[Text](03-text.md), its array methods in [Arrays](04-arrays.md), and its maps, sets and
iterators in [Maps, sets, pools and iterators](09-collections.md). The data packages are
in [The library: data](14-library-data.md).

The library is Zinkite, in `compiler/lib/zink/`, built into the `zink` binary. A program
gets only the functions it calls. The design, with the reasons for each decision, is
[`STDLIB.md`](../STDLIB.md); this chapter describes what is written.

| Package | Imported | What it holds |
|---|---|---|
| `zink.lang` | always | `env`, `EnvVar`, `status`, `stdin`, `stdout`, `stderr`, `print` and the other output functions |
| `zink.io` | `import zink.io.*;` | `File`, `Dir`, `DirEntry`, `Stat`, `fs`, `Walker`, `WalkEntry`, `path` |
| `zink.os` | `import zink.os.*;` | `process`, `Command`, `Child`, `Exit`, `term` |
| `zink.time` | `import zink.time.*;` | `time`, `Duration`, `Instant`, `Timestamp`, `DateTime` |
| `zink.cli` | `import zink.cli.*;` | `Args`, `ArgIterator`, `ansi`, `Colors` |

`stdin`, `stdout` and `stderr` work in every program, since they are `File`s and `zink.io`
is in every program. Naming the type `File`, or using `fs` and `path`, takes
`import zink.io.*;`.

## How the library behaves

These rules hold for every package in this chapter and the next
([STDLIB §1.3](../STDLIB.md#13-conventions)).

- **Every failure is a status** ([Errors and statuses](07-errors.md)). Operating-system
  errors pass through as their `errno` number, so a missing file is `status.NOT_FOUND`
  (2) and a full disk `status.NO_SPACE` (28). The tables below list the codes worth
  handling; a call that wraps a system call can also fail with any other `errno` the
  kernel returns.
- **A failure nothing reads stops the program at the line of the call**
  ([A failure nothing reads stops the program](07-errors.md#a-failure-nothing-reads-stops-the-program)). So
  `File f = File.open(path);` is fine in a script that can't go on without the file, and
  `File f @err = File.open(path);` is the form for a failure the program handles.
- **The value on failure is the zero value**: an empty text, 0, `false`, or an object
  whose fields are zero.
- **The library doesn't stop the program itself**, except for an arithmetic overflow,
  such as a `Duration` past 292 years. An argument a function can't take fails with
  `INVALID`, which stops the program only if nothing reads it.
- **Text read in one go is built in place**, where the caller keeps it, as a `u8[.]`:
  `u8[.] text = File.readAll(path);`. Each such call also has a form that appends to a
  buffer the caller passes, a `u8[..>]&`, for reusing a buffer or adding to text already
  there. A buffer is a growable array, `u8[..>]` or `u8[..~]`.
- **Results that point into text are views** of it, rooted there
  ([Memory and references](05-memory.md)): `path.base(p)` is part of `p`, and the values
  `Args` gives are parts of the arguments it parsed.
- **A class with only static members has a lowercase name** (`fs`, `path`, `env`, `time`),
  and reads as a namespace. A local variable with the same name hides it inside its scope.

## The environment: `env`

`env` reads and changes the program's environment variables, which child processes
inherit.

```java
import zink.os.*;

void main() {
    string editor @unset = env.get("EXAMPLE_EDITOR");
    println("before: '${editor}', ${status.name(unset)}");

    env.set("EXAMPLE_EDITOR", "vi");
    string first = env.get("EXAMPLE_EDITOR");
    env.set("EXAMPLE_EDITOR", "nano");
    println("now ${env.get("EXAMPLE_EDITOR")}; the first view still says ${first}");

    u8[..>] out = [];
    process.run("sh", ["-c", "echo \"a child sees $EXAMPLE_EDITOR\""], out);
    print(out);

    env.set("EXAMPLE_PAGER", "less");
    for (EnvVar v : env.all()) {
        if (v.name.startsWith("EXAMPLE_")) {
            println("${v.name}=${v.value}");
        }
    }
    env.unset("EXAMPLE_EDITOR");
    println("after unset: ${env.has("EXAMPLE_EDITOR")}");
    u8 bad = @env.set("A=B", "x");
    println("a name with '=': ${status.name(bad)}");
}
```

```output
before: '', MISSING
now nano; the first view still says vi
a child sees nano
EXAMPLE_EDITOR=nano
EXAMPLE_PAGER=less
after unset: false
a name with '=': INVALID
```

```java
string env.get(string name)               // MISSING when it isn't set
bool env.has(string name)
void env.set(string name, string value)   // INVALID: a name that is empty or has '='
void env.unset(string name)               // never fails; unsetting a missing name is fine
EnvIterator env.all()                     // every variable: for (EnvVar v : env.all())

class EnvVar { string name; string value; }
```

- **Views from `get` and `all` are rooted at `static`.** The runtime never frees
  environment text, so a view taken before a `set` or an `unset` keeps the old value.
- **A variable set to `""` exists**: `get` gives `""` with no status, and `has` is true.
- **`all` gives the variables in the environment's order.** A variable `set` for the first
  time comes after the ones that were there.
- **Child processes inherit the changes**, whether started by `process.run` or `Command`.
  `Command` can also change the environment of one child only (below).

## Statuses as text: `status.message` and `status.name`

A program that reports a failure usually prints `status.message`. The status codes, and
the `@` forms that read them, are in [Errors and statuses](07-errors.md).

```java
import zink.io.*;

void main() {
    File f @err = File.open("/no/such/file");
    println("${err} ${status.name(err)}: ${status.message(err)}");
    println("${status.name(status.END)}: ${status.message(status.END)}");
    println("[${status.name(230)}] ${status.message(230)}");
}
```

```output
2 NOT_FOUND: No such file or directory
END: End of input
[] Error 230
```

```java
string status.message(u8 code)   // "No such file or directory"; "Error 230" for a program's own codes
string status.name(u8 code)      // "NOT_FOUND"; the C name, such as "ESRCH", for an errno without one; "" for 224-255
```

## The standard streams: `stdin`, `stdout` and `stderr`

`stdin`, `stdout` and `stderr` are `File`s for descriptors 0, 1 and 2, with every method
of `File` below. A `File` variable can hold any of them, so a program that reads a file
or its standard input needs one loop, a [repeat loop](07-errors.md#repeat-loops) here:

```java
import zink.io.*;

// number: print each line of a file, or of stdin, with its number.
u8 main(string[.] args) {
    File input = stdin;
    if (args.len > 0) {
        File f @err = File.open(args[0]);
        if (err) {
            errprintln("number: can't open ${args[0]}: ${status.message(err)}");
            return 1;
        }
        input = f;
    }
    u8[..~] buf = [];
    i64 n = 0;
    while (string line : input.nextLine(buf)) @why {
        n++;
        println("${n.pad(4)}  ${line}");
    }
    if (why != status.END) {
        errprintln("number: ${status.message(why)}");
        return 1;
    }
    return 0;
}
```

```
$ printf 'alpha\nbeta\r\ngamma' | zink number.zn
   1  alpha
   2  beta
   3  gamma
$ zink number.zn /nope
number: can't open /nope: No such file or directory
```

- **Writing through `stdout` or `stderr` goes where `print` and `errprint` go**, in order
  with them. stdout is C's `stdout`: line-buffered on a terminal, and buffered 64 KB at a
  time otherwise. stderr is unbuffered, and a write to it writes what stdout holds first,
  so the two stay in order when they go to the same place.
- **`write`, `print` and `println` on `stdout` and `stderr` never fail.** A failed write to
  stdout shows at `stdout.flush()`, which fails with its `errno` (`NO_SPACE` on a full
  disk), or at exit, where it is reported on stderr and makes the exit status 1. The
  global `print` and `println` set no status at all
  ([STDLIB §2.1](../STDLIB.md#21-output-and-streams)).
- **Writing to stdout after its reader has gone ends the program quietly**, with exit
  status 141, as `head` expects of the programs it reads from.
- **`stdin` reads lines through a 64 KB buffer**, as every `File` does, so `nextLine` and
  `readLine` cost a system call per 64 KB, not per line.

## Files: `File`

A `File` is an open file: a handle ([Calling C](12-calling-c.md#handles) describes handles). It
is opened by one of five static functions, read and written by its methods, and closed by
`close`.

```java
import zink.io.*;

void main() {
    u8[..>] dir = [];
    fs.tempDir(dir);
    u8[.] notes = path.join(dir, "notes.txt");

    File f = File.create(notes);
    f.println("first line");
    f.write("second line\r\n");
    f.print("no newline at the end");
    f.close();

    File g = File.open(notes);
    u8[..~] buf = [];
    while (string line : g.nextLine(buf)) {
        println("[${line}]");
    }
    g.close();

    fs.removeAll(dir);
}
```

```output
[first line]
[second line]
[no newline at the end]
```

### Opening

| Call | Opens the file | Fails with |
|---|---|---|
| `File.open(string path)` | for reading | `NOT_FOUND`, `PERMISSION` |
| `File.create(string path)` | for writing, created or truncated, mode 0666 less the umask | `NOT_FOUND` (a missing directory), `PERMISSION`, `IS_DIRECTORY` |
| `File.append(string path)` | for writing at the end, created if missing | as `create` |
| `File.createNew(string path)` | for writing; it must not exist yet (`O_EXCL`), so it works as a lock file | `EXISTS`, and as `create` |
| `File.update(string path)` | for reading and writing, without truncating | `NOT_FOUND`, `PERMISSION` |

- **A failed open gives a `File` that is already closed**, so each of its methods fails
  with `BAD_HANDLE`.
- **`File.open` of a directory succeeds**; reading it then fails with `IS_DIRECTORY`.
- **Files are opened with `O_CLOEXEC`**, so a child process doesn't inherit them.

### Reading

```java
i64 f.read(u8[..>]& buf, i64 max)     // up to max bytes, appended; how many; END at the end of the file
void f.readLine(u8[..>]& buf)         // one line, appended, without its \n or \r\n; END when nothing was left
string f.nextLine(u8[..~]& buf)       // clears buf, reads one line into it, and gives a view of it; END as readLine
u8[.] f.readAll()                     // the rest of the file
void f.readAll(u8[..>]& buf)          // the rest of the file, appended to buf
```

```java
import zink.io.*;

void main() {
    u8[..>] dir = [];
    fs.tempDir(dir);
    u8[.] p = path.join(dir, "data.txt");
    File.writeAll(p, "alpha\nbeta\r\ngamma");

    File f = File.open(p);
    u8[..>] buf = [];
    f.readLine(buf);
    f.readLine(buf);
    println("two lines appended: '${buf}'");
    u8[..>] chunk = [];
    i64 n = f.read(chunk, 3);
    println("read ${n}: '${chunk}', at ${f.position()} of ${f.size()}");
    u8[.] rest = f.readAll();
    println("the rest: '${rest}'");
    i64 none @end = f.read(chunk, 10);
    u8 noLine = @f.readLine(buf);
    u8[.] empty @atEnd = f.readAll();
    println("at the end: read ${none} ${status.name(end)}, readLine ${status.name(noLine)}, readAll '${empty}' ${status.name(atEnd)}");
    f.close();
    fs.removeAll(dir);
}
```

```output
two lines appended: 'alphabeta'
read 3: 'gam', at 15 of 17
the rest: 'ma'
at the end: read 0 END, readLine END, readAll '' OK
```

- **`readLine` keeps a last line that has no newline**, and fails with `END` only when
  nothing at all was left. A `\r` before the `\n` is removed too.
- **`nextLine` is for repeat loops**: `while (string line : f.nextLine(buf))` reads every
  line into one reused buffer. Its buffer must allow removal (`u8[..~]`), since each call
  clears it. Clearing it is safe because the previous pass's `line` is gone when the next
  call runs; a view kept from an earlier pass is a compile error:

```java
void main() {
    u8[..~] buf = [];
    string first = "";
    while (string line : stdin.nextLine(buf)) {
        if (first.len == 0) {
            first = line;
        }
    }
    println(first);
}
```

```error
`nextLine(...)` removes from `buf` while `first` may still refer into it
```

- **To keep every line, append them all to one buffer** with `readLine`, and keep views
  into it. Appending never moves the lines already there, so the views stay valid:

```java
import zink.io.*;

void main() {
    u8[..>] dir = [];
    fs.tempDir(dir);
    u8[.] p = path.join(dir, "names.txt");
    File.writeAll(p, "carol\nann\nbob\n");

    File f = File.open(p);
    u8[..>] text = [];              // every line, end to end
    string[..~] lines = [];         // views into text
    while (true) {
        i64 start = text.len;
        u8 st = @f.readLine(text);
        if (st == status.END) {
            break;
        }
        if (st != 0) {
            errprintln("can't read ${p}: ${status.message(st)}");
            break;
        }
        lines +> text[start:];
    }
    f.close();
    lines.sort();
    println("${lines}, from ${text.len} bytes");
    fs.removeAll(dir);
}
```

```output
[ann, bob, carol], from 11 bytes
```

- **`read` with a `max` of 0 reads nothing and succeeds.** With a positive `max` it fails
  with `END` only at the end of the file; a short read is not the end.
- **`readAll` at the end of the file succeeds** and gives nothing. On a failure,
  `f.readAll()` gives an empty text, and `f.readAll(buf)` leaves in `buf` what it read
  before the failure.

### Writing

```java
void f.write(string bytes)            // IO, NO_SPACE, BROKEN_PIPE, BAD_HANDLE
void f.print(string text)             // the same as write
void f.println(string text)           // text and a newline
void f.flush()                        // writes what is buffered: IO, NO_SPACE
void f.sync()                         // flush, then fsync
void f.close()                        // flush, then close: BAD_HANDLE when already closed
```

- **Writes are buffered, 64 KB at a time.** The buffer is written when it fills, by
  `flush`, `sync` and `close`, before a read, a seek, or asking the size or position, and at
  exit, however the program ends, a bug that stops it included.
- **So a failed write shows at one of those calls**, not at the `write` that put the bytes
  in the buffer. Apart from `BAD_HANDLE`, a `write` fails only when its bytes made the
  buffer write out.
- **A failed `flush` drops what was buffered**, so a `close` after it succeeds.
- **A file left open at exit is written out, but a failure then isn't reported.** Close a
  file the program wrote, and read what `close` says.

```java
import zink.io.*;

void main() {
    u8[..>] dir = [];
    fs.tempDir(dir);
    u8[.] p = path.join(dir, "log.txt");

    File f = File.create(p);
    f.println("written");
    println("before flush: ${File.readAll(p).len} bytes on disk");
    f.flush();
    println("after flush: ${File.readAll(p).len} bytes on disk");
    f.println("more");
    println("size ${f.size()}, position ${f.position()}");
    f.close();

    File full = File.create("/dev/full");
    full.println("no room");
    u8 err = @full.close();
    println("closing /dev/full: ${status.name(err)}, ${status.message(err)}");
    fs.removeAll(dir);
}
```

```output
before flush: 0 bytes on disk
after flush: 8 bytes on disk
size 13, position 13
closing /dev/full: NO_SPACE, No space left on device
```

### Position and type

```java
void f.seek(i64 pos)                  // from the start: ESPIPE on a pipe, INVALID for a negative pos
i64 f.position()                      // ESPIPE on a pipe
i64 f.size()
bool f.isTerminal()                   // never fails; false for a closed File
```

- **`position` counts what is read and written**: bytes read ahead into the input buffer
  but not yet returned don't count, and buffered writes are written first.
- **`seek` drops the input buffer**, so the next read starts at `pos`.
- **`File.update` with `seek`** changes a file in place:

```java
import zink.io.*;

void main() {
    u8[..>] dir = [];
    fs.tempDir(dir);
    u8[.] p = path.join(dir, "data.txt");
    File.writeAll(p, "abc");

    File u = File.update(p);
    u.seek(1);
    u.write("Z");
    u.seek(0);
    println("${u.readAll()}");
    u.close();

    File a = File.append(p);
    a.println("!");
    println("appended, now at ${a.position()}");
    a.close();
    print(File.readAll(p));
    fs.removeAll(dir);
}
```

```output
aZc
appended, now at 5
aZc!
```

### Whole files

```java
u8[.] File.readAll(string path)                 // open, read to the end, close: NOT_FOUND, PERMISSION, IS_DIRECTORY
void File.readAll(string path, u8[..>]& buf)    // the same, appended to buf
void File.writeAll(string path, string bytes)   // replaces the file atomically: NOT_FOUND, PERMISSION
void File.appendAll(string path, string bytes)  // adds to the end, creating the file if missing
```

- **`writeAll` writes a temporary file next to the target, then renames it over the
  target.**
  - Another process sees the old contents or the new ones, never half a file.
  - The old file's permissions are kept; a new file gets mode 0666 less the umask.
  - A symbolic link is followed, so its target is the file that gets replaced, and the
    link stays a link.
  - The temporary file needs write permission on the directory, and is removed if
    anything fails.
  - The new file replaces only the name: another hard link to the old file keeps the old
    contents.
- **`appendAll` opens with `O_APPEND`**, so the bytes go at the end even when another
  process appends too.

```java
import zink.io.*;

void main() {
    u8[..>] dir = [];
    fs.tempDir(dir);
    u8[.] real = path.join(dir, "real.conf");
    u8[.] link = path.join(dir, "app.conf");
    File.writeAll(real, "old\n");
    fs.chmod(real, 416);                        // 0640
    fs.symlink("real.conf", link);

    File.writeAll(link, "new\n");
    File.appendAll(link, "more\n");
    Stat l = fs.lstat(link);
    Stat r = fs.stat(real);
    println("still a link: ${l.isLink}, mode ${r.mode.oct()}");
    print(File.readAll(real));

    u8 err = @File.writeAll(path.join(dir, "no/such/dir.txt"), "x");
    println("into a missing directory: ${status.name(err)}");
    fs.removeAll(dir);
}
```

```output
still a link: true, mode 640
new
more
into a missing directory: NOT_FOUND
```

### Closing, and copies of a `File`

A `File` is a handle, so a copy, such as `File input = f;`, shares the open file. Once one
copy closes it, every copy fails with `BAD_HANDLE`, even after the system gives the same
descriptor number to a file opened later: the handle carries a generation as well as the
descriptor.

```java
import zink.io.*;

void main() {
    u8[..>] dir = [];
    fs.tempDir(dir);
    u8[.] p = path.join(dir, "a.txt");
    File.writeAll(p, "one\ntwo\n");

    File f = File.open(p);
    File copy = f;
    f.close();
    u8 twice = @f.close();
    File other = File.open(p);                  // likely gets the same descriptor
    u8[..>] line = [];
    u8 stale = @copy.readLine(line);
    other.readLine(line);
    println("closed twice: ${status.name(twice)}, the copy: ${status.name(stale)}, the new file: ${line}");
    other.close();
    fs.removeAll(dir);
}
```

```output
closed twice: BAD_HANDLE, the copy: BAD_HANDLE, the new file: one
```

## Directories: `Dir` and `DirEntry`

`Dir.open` opens a directory, and `next` gives its entries one at a time, each name
appended to a buffer the caller passes.

```java
import zink.io.*;

class Item {
    string name;
    bool isDir;
    bool isLink;
}

void main() {
    u8[..>] dir = [];
    fs.tempDir(dir);
    fs.mkdir(path.join(dir, "src"));
    File.writeAll(path.join(dir, "README"), "hello\n");
    File.writeAll(path.join(dir, "notes.txt"), "abc");
    fs.symlink("README", path.join(dir, "link"));

    Dir d = Dir.open(dir);
    u8[..>] names = [];
    Item[..~] items = [];
    while (DirEntry e : d.next(names)) {
        items +> new Item(e.name, e.isDir, e.isLink);
    }
    d.close();
    items.sortBy((Item i) => i.name);
    for (Item& i : items) {
        println("${i.name}: directory ${i.isDir}, link ${i.isLink}");
    }
    fs.removeAll(dir);
}
```

```output
README: directory false, link false
link: directory false, link true
notes.txt: directory false, link false
src: directory true, link false
```

```java
Dir Dir.open(string path)             // NOT_FOUND, PERMISSION, NOT_DIRECTORY
DirEntry d.next(u8[..>]& names)       // END after the last entry; its name is appended to names
void d.close()                        // BAD_HANDLE when already closed

class DirEntry { string name; bool isDir; bool isLink; i64 size; }
```

- **`.` and `..` are skipped.**
- **`e.name` is a view into `names`**, which `next` only appends to, so every name stays
  valid while `names` does. Declare `names` inside a loop over directories to free it each pass, as
  `examples/dirwalk.zn` does.
- **The order is the file system's**, as `ls -U` shows it. Sort the names if order matters.
- **`isDir` and `isLink` describe the entry itself**: a link to a directory has `isLink`
  set and `isDir` clear. `size` is what `lstat` says, so a link's size is the length of
  its target's name. It costs one `fstatat` per entry.
- **A `Dir` is a handle, closed as a `File` is**: `next` on a closed one fails with
  `BAD_HANDLE`. `defer @_ = d.close();` closes it on every way out of a block.

## File information: `Stat`, `fs.stat` and `fs.lstat`

```java
Stat fs.stat(string path)             // follows symbolic links: NOT_FOUND, PERMISSION
Stat fs.lstat(string path)            // describes a link itself

class Stat { i64 size; Timestamp modified; u32 mode; u32 uid; u32 gid; bool isDir; bool isFile; bool isLink; }
```

- **`mode` holds the permission bits only**, `st_mode & 07777`, so 0644 is 420. The kind
  is in `isDir`, `isFile` and `isLink`. A device or a pipe has none of them set.
- **`modified` is a `Timestamp`** (below), with nanoseconds.
- **`fs.stat` of a dangling link fails with `NOT_FOUND`**; `fs.lstat` describes the link.
- **There are no octal literals.** Write a mode in decimal with the octal in a comment
  (`fs.chmod(p, 384);  // 0600`), and print one with `oct()`
  ([Text](03-text.md#formatting-numbers)):

```java
import zink.io.*;

void main() {
    Stat n = fs.stat("/dev/null");
    println("/dev/null: dir ${n.isDir}, file ${n.isFile}, link ${n.isLink}, mode ${n.mode.oct()}");
    Stat m @err = fs.stat("/no/such/file");
    println("missing: ${status.name(err)}, size ${m.size}, modified ${m.modified.nanos}");
}
```

```output
/dev/null: dir false, file false, link false, mode 666
missing: NOT_FOUND, size 0, modified 0
```

## File system operations: `fs`

```java
bool fs.exists(string path)                 // follows links; false for any failure
void fs.remove(string path)                 // a file, a link or an empty directory: NOT_FOUND, NOT_EMPTY, PERMISSION
void fs.removeAll(string path)              // recursively, never following links; a missing path is success
void fs.rename(string from, string to)      // atomic: CROSS_DEVICE across file systems
void fs.move(string from, string to)        // rename, or across file systems a copy and a remove
void fs.copy(string from, string to)        // contents and mode: IS_DIRECTORY for a directory
void fs.mkdir(string path)                  // mode 0777 less the umask: EXISTS, NOT_FOUND (a missing parent)
void fs.mkdirAll(string path)               // creates missing parents; an existing directory is success
void fs.chmod(string path, u32 mode)
void fs.symlink(string target, string link) // EXISTS when link exists
u8[.] fs.readLink(string path)              // where a link points: INVALID when path isn't a link
void fs.readLink(string path, u8[..>]& buf) // the same, appended to buf
File fs.tempFile(u8[..>]& path)             // a new file, mode 0600; its path is appended to path
void fs.tempDir(u8[..>]& path)              // a new directory, mode 0700; its path is appended to path
Walker fs.walk(string root, u8[..~]& path)  // below
```

```java
import zink.io.*;

void main() {
    u8[..>] dir = [];
    fs.tempDir(dir);
    u8[.] a = path.join(dir, "a.txt");
    u8[.] deep = path.join(dir, "x/y/z");
    u8[.] link = path.join(dir, "link");

    File.writeAll(a, "abc");
    fs.symlink("a.txt", link);
    fs.symlink("nowhere", path.join(dir, "dangling"));
    println("exists: ${fs.exists(a)} ${fs.exists(link)} ${fs.exists(path.join(dir, "dangling"))}");
    println("the link points to ${fs.readLink(link)}");
    u8[.] notLink @e1 = fs.readLink(a);
    println("readLink of a file: ${status.name(e1)}");

    u8 e2 = @fs.mkdir(deep);
    fs.mkdirAll(deep);
    u8 e3 = @fs.mkdirAll(deep);
    u8 e4 = @fs.mkdirAll(a);
    println("mkdir with a missing parent: ${status.name(e2)}; mkdirAll again: ${status.name(e3)}; over a file: ${status.name(e4)}");

    u8[.] b = path.join(dir, "b.txt");
    fs.copy(a, b);
    fs.move(b, path.join(dir, "x/b.txt"));
    u8 e5 = @fs.copy(deep, b);
    u8 e6 = @fs.remove(path.join(dir, "x"));
    println("copying a directory: ${status.name(e5)}; removing a full one: ${status.name(e6)}");
    fs.removeAll(path.join(dir, "x"));
    u8 e7 = @fs.removeAll(path.join(dir, "x"));
    println("removeAll of what is gone: ${status.name(e7)}");
    fs.removeAll(dir);
    println("all gone: ${!fs.exists(dir)}");
}
```

```output
exists: true true false
the link points to a.txt
readLink of a file: INVALID
mkdir with a missing parent: NOT_FOUND; mkdirAll again: OK; over a file: NOT_DIRECTORY
copying a directory: IS_DIRECTORY; removing a full one: NOT_EMPTY
removeAll of what is gone: OK
all gone: true
```

- **`removeAll` never follows a link**: a link to a directory is removed, and what it
  points at is kept. It keeps its own stack of directories instead of recursing.
- **`copy` follows a link at `from`**, copies the contents with `copy_file_range` where the
  kernel can (read and write where it can't), and gives `to` the mode of `from`. An
  existing `to` is truncated and replaced.
- **`move` renames when it can.** Across file systems, `rename` fails with
  `CROSS_DEVICE`, and `move` copies the file and removes the original; a link is moved as
  a link. **A directory across file systems isn't written yet**: `move` fails with
  `CROSS_DEVICE` for it.
- **`mkdirAll` fails with `NOT_DIRECTORY`** when the path, or one of its parents, is a
  file.
- **Temporary names are random**, `zink-` and 16 hexadecimal digits from `getrandom`, in
  `$TMPDIR`, or `/tmp` when it isn't set. They are created with `O_EXCL`, so two programs
  never get the same one. `tempFile` gives the file open for reading and writing.
- **Removing them is the caller's job**: `defer @_ = fs.remove(tmp);` for a file, and
  `defer @_ = fs.removeAll(dir);` for a directory.

```java
import zink.io.*;

u8 main() {
    u8[..>] tmp = [];
    File f @err = fs.tempFile(tmp);
    if (err) {
        errprintln("can't make a temporary file: ${status.message(err)}");
        return 1;
    }
    defer @_ = fs.remove(tmp);
    f.println("scratch");
    f.seek(0);
    u8[.] back = f.readAll();
    f.close();
    Stat s = fs.stat(tmp);
    println("read back '${back.trim()}', mode ${s.mode.oct()}, named ${path.base(tmp).startsWith("zink-")}");
    return 0;
}
```

```output
read back 'scratch', mode 600, named true
```

## Walking a tree: `fs.walk`, `Walker` and `WalkEntry`

`fs.walk` gives every file and directory under a root, one per call to `next`, keeping only
the current path in a buffer the caller passes.

```java
import zink.io.*;

void make(string root, string rel, string text) {
    u8[.] p = path.join(root, rel);
    fs.mkdirAll(path.dir(p));
    File.writeAll(p, text);
}

void main() {
    u8[..>] root = [];
    fs.tempDir(root);
    make(root, "src/main.zn", "void main() {}\n");
    make(root, "src/lib/util.zn", "// util\n");
    make(root, "src/lib/notes.txt", "notes\n");
    make(root, ".git/HEAD", "ref\n");
    fs.symlink("src", path.join(root, "alias"));

    u8[..~] buf = [];
    u8[..>] kept = [];              // copies of what the loop keeps
    string[..~] lines = [];
    Walker w = fs.walk(root, buf);
    while (WalkEntry e : w.next()) {
        if (e.isDir && e.name == ".git") {
            w.skip();
            continue;
        }
        if (e.err != 0) {
            errprintln("can't read ${e.path}: ${status.message(e.err)}");
            continue;
        }
        if (e.depth > 0) {
            i64 start = kept.len;
            kept.append("${e.path[root.len + 1:]}${e.isDir ? "/" : ""}${e.isLink ? " (a link)" : ""} at depth ${e.depth}");
            lines +> kept[start:];
        }
    }
    lines.sort();
    for (string l : lines) {
        println(l);
    }
    fs.removeAll(root);
}
```

```output
alias (a link) at depth 1
src/ at depth 1
src/lib/ at depth 2
src/lib/notes.txt at depth 3
src/lib/util.zn at depth 3
src/main.zn at depth 2
```

```java
Walker fs.walk(string root, u8[..~]& path)
WalkEntry w.next()          // END after the last entry
void w.skip()               // don't go into the directory next() just returned

class Walker { bool followLinks = false; i64 maxDepth = i64.MAX; ... }
class WalkEntry { string path; string name; i64 depth; bool isDir; bool isLink; i64 size; u8 err; }
```

- **The walk is depth-first, and each directory comes before what it holds.** The root
  comes first, at depth 0, with `name` its last component. Within a directory the order
  is the file system's; sort what the walk gives if order matters.
- **`path` holds only the current path.** The walker clears it, appends the root, and then
  appends each name and takes it off again, so the buffer never grows past the deepest
  path. `e.path` and `e.name` are views into it, valid until the next step. Keeping one
  longer is a compile error; append it to a buffer of your own instead, as `kept` above.
- **A failure is an entry, not the end of the walk.** A directory that can't be read is
  still returned, with `err` set to the reason, and isn't gone into. A root that doesn't
  exist is one entry with `err` set to `NOT_FOUND`. The loop ends only with `END`.
- **Symbolic links aren't followed** unless `w.followLinks` is set. A link is returned
  with `isLink` set; with `followLinks`, a link to a directory also has `isDir` set and is
  gone into. A loop, a link to a directory the walk is already in, is detected by device
  and inode and returned with `err` set to `LOOP`.
- **`w.maxDepth`** stops the walk going below a depth: a directory at `maxDepth` is
  returned but not opened.
- **At most 256 levels are open at once.** A directory deeper than that is returned with
  `err` set to `NAME_TOO_LONG`, and not gone into.
- **`w.skip()` acts on the walker the loop calls**, which is why the loop is a repeat loop
  that names it. `for (WalkEntry e : w)` is an error that suggests that form.

```java
import zink.io.*;

void walk(string root, bool follow, i64 depth) {
    u8[..~] buf = [];
    u8[..>] kept = [];
    string[..~] lines = [];
    Walker w = fs.walk(root, buf);
    w.followLinks = follow;
    w.maxDepth = depth;
    while (WalkEntry e : w.next()) {
        i64 start = kept.len;
        string rel = e.depth == 0 ? "." : e.path[root.len + 1:];
        kept.append("${rel} (depth ${e.depth}, dir ${e.isDir}, link ${e.isLink}, ${status.name(e.err)})");
        lines +> kept[start:];
    }
    lines.sort();
    for (string l : lines) {
        println("  ${l}");
    }
}

void main() {
    u8[..>] root = [];
    fs.tempDir(root);
    fs.mkdirAll(path.join(root, "a/b"));
    File.writeAll(path.join(root, "a/b/f.txt"), "x");
    fs.symlink("..", path.join(root, "a/up"));
    println("maxDepth 1:");
    walk(root, false, 1);
    println("followLinks:");
    walk(root, true, i64.MAX);
    println("a missing root:");
    walk(path.join(root, "nope"), false, i64.MAX);
    fs.removeAll(root);
}
```

```output
maxDepth 1:
  . (depth 0, dir true, link false, OK)
  a (depth 1, dir true, link false, OK)
followLinks:
  . (depth 0, dir true, link false, OK)
  a (depth 1, dir true, link false, OK)
  a/b (depth 2, dir true, link false, OK)
  a/b/f.txt (depth 3, dir false, link false, OK)
  a/up (depth 2, dir true, link true, LOOP)
a missing root:
  . (depth 0, dir false, link false, NOT_FOUND)
```

## Paths: `path`

`path` works on paths as text. Paths are bytes, as on Linux, and needn't be valid UTF-8.

```java
import zink.io.*;

void main() {
    string[.] ps = ["dir/file.tar.gz", "file.txt", "/etc/hosts", "a/b//", "/", ".bashrc", ""];
    for (string p : ps) {
        println("'${p}': base '${path.base(p)}', dir '${path.dir(p)}', ext '${path.ext(p)}', stem '${path.stem(p)}'");
    }
    println("${path.join("a", "b")} ${path.join("a/", "b")} ${path.join("a", "/b")} ${path.join("a/b", "../c")}");
    string[.] ns = ["a/./b/../c/", "a/../..", "/..", "", "//x//y/.", "../../a/../b"];
    for (string n : ns) {
        print("'${path.normalize(n)}' ");
    }
    println("");
    println("${path.relative("a/b", "a/c/d")} ${path.relative("src", "src/lib/x.zn")} ${path.relative("/x/y", "/")}");
    u8[.] r @bad = path.relative("a", "/b");
    println("relative of a relative and an absolute path: '${r}' ${status.name(bad)}");
}
```

```output
'dir/file.tar.gz': base 'file.tar.gz', dir 'dir', ext '.gz', stem 'file.tar'
'file.txt': base 'file.txt', dir '.', ext '.txt', stem 'file'
'/etc/hosts': base 'hosts', dir '/etc', ext '', stem 'hosts'
'a/b//': base 'b', dir 'a', ext '', stem 'b'
'/': base '/', dir '/', ext '', stem '/'
'.bashrc': base '.bashrc', dir '.', ext '', stem '.bashrc'
'': base '', dir '.', ext '', stem ''
a/b a/b /b a/b/../c
'a/c' '..' '/' '.' '/x/y' '../../b' 
../c/d lib/x.zn ../..
relative of a relative and an absolute path: '' INVALID
```

```java
u8[.] path.join(string a, string b)         // "a/b"; an absolute b replaces a; "" for either gives the other
string path.base(string p)                  // the last component, without trailing slashes
string path.dir(string p)                   // everything before it: "." when there's no slash
string path.ext(string p)                   // ".gz"; "" when the base has no dot, or only a leading one
string path.stem(string p)                  // the base without its ext
bool path.isAbsolute(string p)              // starts with "/"
u8[.] path.normalize(string p)              // removes ".", "..", repeated and trailing slashes; "" gives "."
u8[.] path.absolute(string p)               // joined to the working directory, then normalized
u8[.] path.relative(string from, string to) // the way from one to the other: INVALID (below)
u8[.] path.resolve(string p)                // follows symbolic links (realpath): NOT_FOUND
u8[.] path.cwd()                            // the working directory
void path.cwd(u8[..>]& buf)                 // the same, appended to buf
void path.chdir(string p)                   // NOT_FOUND, NOT_DIRECTORY, PERMISSION
```

- **Everything except `absolute`, `resolve`, `cwd` and `chdir` is lexical**, and never
  touches the file system. So `normalize("a/../..")` is `".."` and `normalize("/..")` is
  `"/"`, whatever the links on the way are; `resolve` is the one that knows.
- **`base`, `dir`, `ext` and `stem` return views** into `p`, or the literal `"."`.
- **`join` doesn't normalize**: `path.join("a/b", "../c")` is `a/b/../c`.
- **`relative` fails with `INVALID`** when one path is absolute and the other isn't, or
  when the way would have to climb out of a `..` that `from` starts with, as from `../a`
  to `b`.
- **`chdir` changes the working directory of the whole program**, which relative paths in
  every later call are resolved against, and which child processes start in.

```java
import zink.io.*;

void main() {
    u8[..>] dir = [];
    fs.tempDir(dir);
    u8[.] real = path.resolve(dir);         // $TMPDIR may itself be a link
    fs.mkdir(path.join(dir, "sub"));
    path.chdir(dir);
    println("cwd: ${path.cwd() == real}");
    println("absolute: ${path.absolute("sub/../x.txt") == path.join(real, "x.txt")}");
    fs.symlink("sub", "link");
    println("resolve: ${path.relative(real, path.resolve("link"))}");
    u8[.] none @notFound = path.resolve("missing");
    u8 bad = @path.chdir("missing");
    println("missing: '${none}' ${status.name(notFound)}, ${status.name(bad)}");
    path.chdir("/");
    fs.removeAll(dir);
}
```

```output
cwd: true
absolute: true
resolve: sub
missing: '' NOT_FOUND, NOT_FOUND
```

## Running programs: `process`

`process.run` runs a program to its end and collects its standard output.

```java
import zink.os.*;

void main() {
    u8[..>] out = [];
    Exit e @err = process.run("sh", ["-c", "echo hello; echo \"$1 | $2\"", "sh", "two words", "and more"], out);
    print("status ${err}, code ${e.code}, ok ${e.ok()}, output:\n${out}");

    u8[..>] none = [];
    Exit three = process.run("sh", ["-c", "exit 3"], none);
    Exit killed = process.run("sh", ["-c", "kill -TERM $$"], none);
    Exit missing @notFound = process.run("no-such-program-here", [], none);
    println("exit code ${three.code}, ok ${three.ok()}; signal ${killed.signal}; missing: ${status.name(notFound)}");
}
```

```output
status 0, code 0, ok true, output:
hello
two words | and more
exit code 3, ok false; signal 15; missing: NOT_FOUND
```

```java
Exit process.run(string program, string[:] args, u8[..>]& out)  // NOT_FOUND, PERMISSION
void process.exec(string program, string[:] args)                // replaces this process; returns only on failure
i32 process.pid()
string process.name()                                           // argv[0]; main's args don't include it
u8[.] process.which(string program)                             // the full path PATH finds: NOT_FOUND

class Exit { u8 code; u8 signal; }
bool e.ok()                                                     // code 0 and no signal
```

- **A program without a `/` is searched for in `PATH`.** Arguments reach it as they are,
  with no shell in between, so nothing needs quoting. The program's own name, its
  `argv[0]`, is `program`.
- **`run` appends the child's stdout to `out`.** Its stdin and stderr are this program's.
- **A program that runs and fails isn't a failed call.** The status is 0, and `e.code`, or
  `e.signal` when a signal ended it, says how it ended. The status is set only when the
  program couldn't be started: `NOT_FOUND`, or `PERMISSION` for a file that isn't
  executable.
- **The child starts with `posix_spawnp`.** This program's buffered stdout is written
  first, so output stays in order, and SIGPIPE goes back to its default in the child.
- **`exec` replaces the program**, after writing what stdout holds. It returns only when
  the program couldn't be started:

```java
import zink.os.*;

u8 main() {
    println("before exec");
    u8 err = @process.exec("sh", ["-c", "echo \"now $0, exiting 4\"; exit 4"]);
    errprintln("exec failed: ${status.message(err)}");
    return 1;
}
```

```output exit=4
before exec
now sh, exiting 4
```

- **`which` finds what `run` would start**: the first executable regular file in `PATH`,
  or `program` itself when it has a `/`.
- **`pid` and `name` vary from run to run**, so the next example has no output shown.
  Under `zink run`, the name is the path of the cached executable.

```java
import zink.os.*;

void main() {
    u8[.] sh @notFound = process.which("sh");
    if (notFound) {
        errprintln("no sh in PATH");
        return;
    }
    println("sh is ${sh}; this is ${process.name()}, process ${process.pid()}");
}
```

## More control: `Command`, `Child` and `Exit`

`Command` sets up a program step by step: its arguments, its working directory, changes to
its environment, text for its stdin, and a time limit.

```java
import zink.os.*;
import zink.time.*;

void main() {
    Command c = new Command("sh");
    c.arg("-c");
    c.arg("pwd; echo \"$GREETING [$DROPPED]\"; while read line; do echo \"got $line\"; done; echo oops >&2; exit 5");
    c.cwd = "/";
    c.setEnv("GREETING", "hi there");
    c.setEnv("DROPPED", "x");
    c.unsetEnv("DROPPED");
    c.input = "one\ntwo\n";
    u8[..>] out = [];
    u8[..>] errs = [];
    Exit r = c.outputBoth(out, errs);
    print("code ${r.code}\n${out}stderr: ${errs}");

    Command spin = new Command("sh");
    spin.args(["-c", "while :; do :; done"]);
    spin.timeout = Duration.ms(100);
    Exit s @timeout = spin.run();
    println("${status.name(timeout)}, killed by signal ${s.signal}");
}
```

```output
code 5
/
hi there []
got one
got two
stderr: oops
TIMEOUT, killed by signal 9
```

```java
Command c = new Command(string program)
void c.arg(string a)                     // adds one argument
void c.args(string[:] all)               // adds each
void c.setEnv(string name, string value) // for this child only
void c.unsetEnv(string name)
string c.cwd = ""                        // its working directory; "" keeps this program's
string c.input = ""                      // fed to its stdin; when "", stdin is inherited
Duration c.timeout                       // 0, the default, for no limit

Exit c.run()                                    // stdout and stderr inherited
Exit c.output(u8[..>]& out)                     // stdout appended to out
Exit c.outputBoth(u8[..>]& out, u8[..>]& errs)  // stdout and stderr captured separately
Child c.spawn()                                 // runs alongside this program

Exit ch.wait()                                  // waits for it to end: ECHILD when already waited for
void ch.kill()                                  // SIGTERM
void ch.killNow()                               // SIGKILL
```

- **`run`, `output` and `outputBoth` fail as `process.run` does**, `NOT_FOUND` or
  `PERMISSION`, and also with `NOT_FOUND` when `cwd` doesn't exist. They wait for the
  program to end.
- **With a `timeout`, a program still running when it is over is killed with SIGKILL**,
  and the call fails with `TIMEOUT`; `e.signal` is 9.
- **One `poll` loop feeds `input` and reads stdout and stderr**, so a child that writes a
  lot while it is still reading its input can't deadlock against this program.
- **Storage**: the arguments and the environment changes are copied into the command's
  one growable field, so an argument built for the call, such as `c.arg("${n}")`, needn't
  outlive it. `cwd` and `input` are views, so what they view must outlive the command.
- **The last change to a variable wins**: `setEnv` then `unsetEnv` of the same name leaves
  it unset in the child.

### Running alongside: `spawn` and `Child`

`spawn` starts the program and returns at once. The child has this program's stdin,
stdout and stderr; `input` and `timeout` don't apply.

```java
import zink.os.*;

void main() {
    Command c = new Command("sh");
    c.args(["-c", "echo working; exit 7"]);
    Child child = c.spawn();
    Exit done = child.wait();
    println("it exited with ${done.code}");
    Exit again @twice = child.wait();
    println("waiting again: ${status.name(twice)}");

    Command spin = new Command("sh");
    spin.args(["-c", "while :; do :; done"]);
    Child busy = spin.spawn();
    busy.kill();
    println("killed by signal ${busy.wait().signal}");
}
```

```output
working
it exited with 7
waiting again: ECHILD
killed by signal 15
```

- **Wait for every child you spawn.** One that ends before it is waited for stays in the
  process table until the program ends.
- **`kill` and `killNow` fail with `ESRCH`** for a child that has ended and been waited for.

## The terminal: `term`

```java
(i64, i64) term.size()                // (columns, rows) of stdout's terminal: NOT_TERMINAL
bool f.isTerminal()                   // on every File, stdout included
```

```java
import zink.os.*;

void main() {
    (i64 cols, _) @notTty = term.size();
    if (notTty || cols == 0) {
        cols = 80;
    }
    println("${"-".repeat(cols)}");
}
```

- **`term.size` asks about stdout**, so it fails with `NOT_TERMINAL` when stdout is a pipe
  or a file, even if stdin is a terminal.
- **A terminal that reports no size gives `(0, 0)`** with no status, as a pseudo-terminal
  made by `script` can.
- **Colors are in `zink.cli`'s `ansi`** (below).

## Time: `time`, `Duration`, `Instant`, `Timestamp` and `DateTime`

`zink.time` has two clocks, the wall clock for dates and the monotonic one for measuring,
and the classes their readings are.

```java
import zink.time.*;

void main() {
    Timestamp start = time.now();
    Instant t0 = time.monotonic();
    time.sleep(Duration.ms(25));
    Duration took = t0.elapsed();
    println("started at ${start.iso()}");
    println("slept ${took.human()}, ${took.toMs()} ms, ${took.toSeconds()} s");
    DateTime now = time.now().utc();
    println("today is ${now.year}-${now.month.zeroPad(2)}-${now.day.zeroPad(2)}, weekday ${now.weekday()}");
}
```

```
$ zink clock.zn
started at 2026-11-25T17:27:00.646814091Z
slept 25.16ms, 25 ms, 0.02516174 s
today is 2026-11-25, weekday 3
```

```java
Timestamp time.now()                  // the wall clock: nanoseconds since 1970-01-01 UTC
Instant time.monotonic()              // for measuring: never goes backwards
void time.sleep(Duration d)           // the whole duration, through signals; a negative one doesn't sleep
```

### `Duration`

```java
class Duration { i64 nanos; }
Duration Duration.ns(i64 n)           // also Duration.us, .ms, .seconds, .minutes and .hours
i64 d.toMs()                          // whole milliseconds, toward zero
f64 d.toSeconds()
Duration d.plus(Duration other)
Duration d.minus(Duration other)
u8[..~24] d.human()                   // "850ns", "1.5us", "350ms", "1.25s", "7m07s", "2h03m"
```

```java
import zink.time.*;

void main() {
    Duration[.] ds = [Duration.ns(850), Duration.us(1500), Duration.ms(350), Duration.ms(1250),
        Duration.ms(1005), Duration.seconds(427), Duration.minutes(123), Duration.hours(50), Duration.ms(-1500)];
    for (Duration d : ds) {
        println("${d.nanos}: ${d.human()}, ${d.toMs()} ms, ${d.toSeconds()} s");
    }
    Duration a = Duration.minutes(1).plus(Duration.seconds(30));
    println("${a.human()} ${a.minus(Duration.seconds(100)).human()} ${new Duration(42).human()}");
}
```

```output
850: 850ns, 0 ms, 8.5e-7 s
1500000: 1.5ms, 1 ms, 0.0015 s
350000000: 350ms, 350 ms, 0.35 s
1250000000: 1.25s, 1250 ms, 1.25 s
1005000000: 1s, 1005 ms, 1.005 s
427000000000: 7m07s, 427000 ms, 427.0 s
7380000000000: 2h03m, 7380000 ms, 7380.0 s
180000000000000: 50h00m, 180000000 ms, 180000.0 s
-1500000000: -1.5s, -1500 ms, -1.5 s
1m30s -10s 42ns
```

- **`human` keeps two decimals at most, cut off rather than rounded**, and drops trailing
  zeros: 1.005 s is `1s`. From a minute up it gives minutes and seconds, and from an hour
  up hours and minutes.
- **A `Duration` is its public `nanos`**, so `new Duration(n)` and `d.nanos` work too, and
  it covers about 292 years either way.
- **An overflow stops the program**, with a message that names the library's line:
  `Duration.hours(3000000)` stops with `<zink>/zink/time/Duration.zn:12: integer overflow`.

### `Instant`

```java
Duration i.elapsed()                  // since i
Duration a.since(Instant b)           // from b to a
```

- **An `Instant` comes only from `time.monotonic()`.** It means nothing outside this run of
  the program, so it has no public fields.

### `Timestamp`

```java
class Timestamp { i64 nanos; }        // since 1970-01-01T00:00:00Z
Timestamp Timestamp.fromUnix(i64 seconds)
i64 t.unixSeconds()                   // whole seconds, rounded down
DateTime t.utc()                      // the calendar fields in UTC
u8[..~32] t.iso()                     // "2026-11-24T09:30:00Z", with fractional seconds when they aren't zero
```

```java
import zink.time.*;

void main() {
    Timestamp t = Timestamp.fromUnix(1795512600);
    DateTime d = t.utc();
    println("${t.iso()}: ${d.year}-${d.month}-${d.day} at ${d.hour}:${d.minute.zeroPad(2)}, weekday ${d.weekday()}");
    Timestamp later = new Timestamp(t.nanos + Duration.hours(1).nanos + Duration.ms(250).nanos);
    println("${later.iso()}");
    Timestamp before = new Timestamp(-1);
    println("${before.iso()}, unixSeconds ${before.unixSeconds()}");
}
```

```output
2026-11-24T09:30:00Z: 2026-11-24 at 9:30, weekday 2
2026-11-24T10:30:00.25Z
1969-12-31T23:59:59.999999999Z, unixSeconds -1
```

- **A `Timestamp` holds nanoseconds in an `i64`**, so it covers 1677 to 2262. One out of
  that range, such as `Timestamp.fromUnix` of a huge number, stops the program.
- **`Timestamp` has no arithmetic methods**: add a `Duration`'s `nanos` to `t.nanos`, as
  above.
- **The calendar is the proleptic Gregorian one**, computed with Howard Hinnant's
  `days_from_civil` and `civil_from_days`, so dates before 1970 work too.

### `DateTime`

```java
class DateTime { i32 year; u8 month; u8 day; u8 hour; u8 minute; u8 second; u32 nanos; i16 offsetMinutes; u8 parts; }
DateTime DateTime.parse(string s)     // RFC 3339, and its date-only and time-only forms: INVALID
u8[..~40] dt.iso()                    // RFC 3339 text of the parts it has
Timestamp dt.toTimestamp()            // INVALID without a date or an offset
u8 dt.weekday()                       // 1 = Monday ... 7 = Sunday, as in ISO 8601
static u8 DateTime.DATE = 1         // and DateTime.TIME = 2, DateTime.OFFSET = 4: the bits of parts
```

```java
import zink.time.*;

void show(string s) {
    DateTime d @err = DateTime.parse(s);
    if (err) {
        println("'${s}': ${status.name(err)}");
        return;
    }
    Timestamp t @noInstant = d.toTimestamp();
    if (noInstant) {
        println("'${s}': parts ${d.parts}, ${d.iso()}, not an instant");
        return;
    }
    println("'${s}': parts ${d.parts}, ${d.iso()}, ${t.iso()}");
}

void main() {
    show("2026-11-18T09:30:00.25+01:30");
    show("2026-11-18 09:30:00z");
    show("2026-11-18T09:30:00");
    show("2026-11-18");
    show("23:59:60.123");
    show("09:30:00Z");
    show("2023-02-29");
    show("2026-11-18T09:30");
}
```

```output
'2026-11-18T09:30:00.25+01:30': parts 7, 2026-11-18T09:30:00.25+01:30, 2026-11-18T08:00:00.25Z
'2026-11-18 09:30:00z': parts 7, 2026-11-18T09:30:00Z, 2026-11-18T09:30:00Z
'2026-11-18T09:30:00': parts 3, 2026-11-18T09:30:00, not an instant
'2026-11-18': parts 1, 2026-11-18, not an instant
'23:59:60.123': parts 2, 23:59:60.123, not an instant
'09:30:00Z': INVALID
'2023-02-29': INVALID
'2026-11-18T09:30': INVALID
```

- **`parts` says which pieces the text had**: a date, a time and an offset, as
  `DateTime.DATE`, `TIME` and `OFFSET`. A local date or time, without an offset, isn't an
  instant, so `toTimestamp` fails with `INVALID` for it. A `Timestamp`'s `utc()` gives all
  three.
- **`parse` accepts** `t` or a space for the `T`, `z` for the `Z`, a fraction of any
  length (digits past nanoseconds are dropped), and a second of 60 for a leap second. It
  rejects a time-only form with an offset, an offset without its colon, and a date that
  doesn't exist.
- **`offsetMinutes` is the offset east of UTC**: `+01:30` is 90. An offset of 0 is written
  `Z`.
- **`weekday` needs a date**; on a time-only value it means nothing.

## Command-line arguments: `Args`

`Args` declares a program's flags, options and positional arguments, parses `main`'s
arguments against them, and generates `--help`.

```java
import zink.cli.*;

u8 main(string[.] args) {
    Args cli = new Args("csvsort", "Sort CSV rows by one column.");
    cli.flag('n', "numeric", "compare the column as numbers");
    cli.flag('r', "reverse", "sort in descending order");
    cli.option('k', "column", "N", "sort by column N, counting from 1");
    cli.positional("file", "input file; stdin when left out");
    u8 code @stop = cli.parseOrStop(args);
    if (stop) {
        return code;
    }

    bool numeric = cli.has("numeric");
    i64 column @bad = cli.getI64("column", 1);
    if (bad || column < 1) {
        return cli.usageError("--column takes a number from 1");
    }
    string file = cli.get("file", "");
    println("numeric ${numeric}, reverse ${cli.has("reverse")}, column ${column}, file '${file}'");
    return 0;
}
```

```output
numeric false, reverse false, column 1, file ''
```

```
$ zink csvsort.zn -nr -k3 data.csv
numeric true, reverse true, column 3, file 'data.csv'
$ zink csvsort.zn --column=2 -- -odd-name.csv
numeric false, reverse false, column 2, file '-odd-name.csv'
$ zink csvsort.zn -x; echo "exit $?"
csvsort: unknown option -x
usage: csvsort [-n] [-r] [-k N] [file]
exit 2
$ zink csvsort.zn -k 0; echo "exit $?"
csvsort: --column takes a number from 1
usage: csvsort [-n] [-r] [-k N] [file]
exit 2
```

### Declaring

```java
Args cli = new Args(string program, string about)
void cli.flag(u8 letter, string name, string help)                 // letter 0 for none
void cli.option(u8 letter, string name, string meta, string help)  // takes a value, shown as meta
void cli.positional(string name, string help)                      // optional, filled in order
void cli.rest(string name, string help)                            // any number more: [files...]
string cli.version = ""                                             // when set, adds --version
```

- **Each declaration fails with `FULL` past 64**: the declarations sit in a capped table
  inside `Args`, and the values found go in its one growable field.
- **`-h` and `--help` are always there.** A flag declared with the letter `h` can't be
  reached by it: `-h` still asks for help, and `--help` lists both.
- **Positionals are optional.** A program that needs one checks `cli.has(name)` and calls
  `usageError` without it.

### Parsing

```java
u8 cli.parseOrStop(string[:] args)     // for main: the exit status; END after --help or --version, INVALID after a usage error
void cli.parse(string[:] args)         // INVALID for a usage error, which cli.error() describes
string cli.error()                     // what was wrong, after parse failed
u8 cli.usageError(string message)      // prints "program: message" and the usage line on stderr; gives 2
```

- **`parseOrStop` is for `main`**, which is the only place a program can end. After
  `--help` or `--version` it prints what was asked for on stdout and fails with `END`;
  after a usage error it prints the error and the usage line on stderr and fails with
  `INVALID`. Either way its value is the exit status `main` should return: 0 after
  `--help` and `--version`, 2 after a usage error, as getopt-based tools and Python's
  argparse give. With no failure it gives 0.
- **`return cli.usageError("...");`** gives the same for a value the program itself
  rejects.
- **`parse` doesn't print**, so a program can report a usage error its own way, or parse
  several argument lists, such as a subcommand's.
- **Accepted forms**:
  - flags alone or combined: `-n`, `-nr`;
  - an option's value after a space, attached, or after `=`: `-k 3`, `-k3`, `--column 3`,
    `--column=3`;
  - options anywhere among the positionals, until `--`, which ends the options;
  - `-` alone, which is a positional, as for stdin.
- **Usage errors**: an unknown option, an option without its value, a value given to a
  flag (`--dry-run=1`), and an argument past the positionals with no `rest` declared.
- **A negative number looks like an option**: `-5` is an unknown option. Give `--` before
  it.

### Reading

```java
bool cli.has(string name)                      // given at least once
i64 cli.count(string name)                     // how many times: -vvv gives 3
string cli.get(string name, string fallback)   // the last value given
i64 cli.getI64(string name, i64 fallback)      // INVALID, RANGE
f64 cli.getF64(string name, f64 fallback)      // INVALID
ArgIterator cli.getAll(string name)            // every value, in order: -I a -I b
ArgIterator cli.restValues()                   // the arguments after the positionals
```

```java
import zink.cli.*;

void main() {
    string[.] args = ["-vv", "-I", "include", "-Ilib", "--include=vendor", "--scale", "2.5", "build", "a.zn", "b.zn", "-v"];
    Args cli = new Args("tool", "Builds things.");
    cli.flag('v', "verbose", "say more; twice for even more");
    cli.flag(0, "dry-run", "change nothing");
    cli.option('I', "include", "DIR", "search DIR too");
    cli.option(0, "scale", "X", "a factor");
    cli.positional("command", "what to do");
    cli.rest("files", "the files");
    cli.parse(args);

    f64 scale = cli.getF64("scale", 1.0);
    println("verbose ${cli.count("verbose")}, dry-run ${cli.has("dry-run")}, scale ${scale}, command ${cli.get("command", "?")}");
    for (string dir : cli.getAll("include")) {
        print("[${dir}] ");
    }
    for (string f : cli.restValues()) {
        print("<${f}> ");
    }
    println("");
    bool typo @undeclared = cli.has("verbos");
    println("an undeclared name: ${status.name(undeclared)}");
}
```

```output
verbose 3, dry-run false, scale 2.5, command build
[include] [lib] [vendor] <a.zn> <b.zn> 
an undeclared name: INVALID
```

- **Options are looked up by their long name**, and so are positionals and the rest.
- **Asking for a name that was never declared fails with `INVALID`.** Nothing reads that
  status in the usual `if (cli.has("lound"))`, so the program stops there the first time
  the line runs, and a typo shows at once:

```java
import zink.cli.*;

u8 main(string[.] args) {
    Args cli = new Args("greet", "Say hello.");
    cli.flag('l', "loud", "shout");
    u8 code @stop = cli.parseOrStop(args);
    if (stop) {
        return code;
    }
    if (cli.has("lound")) {
        println("HELLO");
    } else {
        println("hello");
    }
    return 0;
}
```

```stderr
example.zn:10: `has` failed with INVALID, and nothing read its status
```

- **The fallback is used only when the option is absent.** A value that doesn't parse gives
  0 with `INVALID`, or `RANGE` for an integer past `i64`.
- **`get` of a flag gives `""`** when it was given, and the fallback when it wasn't.
- **Values are views into `args`**, so nothing is copied, and `cli` can't outlive `args`.
- **`ArgIterator`** is an `Iterator<string>`: loop over it with a for-each, or keep it in a
  variable and call `next()` until `END`.

### The generated help

The usage line lists each declaration in order: `[-n]` for a flag with a letter,
`[--dry-run]` for one without, `[-k N]` for an option, `[file]` for a positional and
`[files...]` for the rest. The help then gives `about`, the flags and options with their
help, `-h, --help`, `--version` when `version` is set, and the positionals. Every help
text starts in the same column.

```java
import zink.cli.*;

void main() {
    Args cli = new Args("tool", "Does things to files.");
    cli.version = "1.2.0";
    cli.flag('v', "verbose", "say more");
    cli.flag(0, "dry-run", "change nothing");
    cli.option('o', "out", "FILE", "write here");
    cli.positional("mode", "what to do");
    cli.rest("files", "the files");
    u8 code @stop = cli.parseOrStop(["--help"]);
    u8 code2 @stop2 = cli.parseOrStop(["--version"]);
    println("${status.name(stop)} ${code}, ${status.name(stop2)} ${code2}");
}
```

```output
usage: tool [-v] [--dry-run] [-o FILE] [mode] [files...]

Does things to files.

  -v, --verbose     say more
      --dry-run     change nothing
  -o, --out FILE    write here
  -h, --help        show this help and exit
      --version     show the version and exit
  mode              what to do
  files...          the files
tool 1.2.0
END 0, END 0
```

## Colors: `ansi` and `Colors`

```java
Colors ansi.colors(File f)     // escape codes when f is a terminal, NO_COLOR isn't set to something, and TERM isn't "dumb"
Colors ansi.always()           // for --color=always
Colors ansi.never()            // for --color=never

class Colors { string bold; string dim; string underline; string red; string green; string yellow;
               string blue; string magenta; string cyan; string gray; string reset; }
```

```java
import zink.cli.*;

void main() {
    Colors c = ansi.colors(stdout);
    println("[${c.red}${c.bold}error:${c.reset} plain in a pipe]");
    Colors on = ansi.always();
    string[.] codes = [on.bold, on.dim, on.underline, on.red, on.green, on.yellow, on.blue, on.magenta, on.cyan, on.gray, on.reset];
    for (string code : codes) {
        print("ESC${code[1:]} ");
    }
    println("");
}
```

```output
[error: plain in a pipe]
ESC[1m ESC[2m ESC[4m ESC[31m ESC[32m ESC[33m ESC[34m ESC[35m ESC[36m ESC[90m ESC[0m 
```

- **When colors are off, every field is `""`**, so the same code prints plain text into a
  pipe or a file.
- **`NO_COLOR` turns them off when it is set to anything but `""`**, as
  [no-color.org](https://no-color.org) says.
- **Ask about the stream the text goes to**: `ansi.colors(stderr)` for error messages.
- **The strings are static**, so a `Colors` is a handful of views.

## What isn't written yet

These are designed in `STDLIB.md` and not in the library yet; [What isn't there
yet](15-not-yet.md#the-library) lists them with the rest.

- **`fs.move` of a directory across file systems** fails with `CROSS_DEVICE`. Copy the
  tree with `fs.walk`, `fs.mkdir` and `fs.copy`, then `fs.removeAll` it.
- **`zink.os`**: pipelines (`a | b`), pipes to and from a spawned child, and signal
  handlers, such as cleanup on Ctrl-C ([STDLIB §5.1](../STDLIB.md#51-running-programs-process-v1-zinkos)).
- **The terminal**: raw mode, reading keys, and reading a password without echo
  ([STDLIB §5.2](../STDLIB.md#52-the-terminal-term-v1-zinkos)).
- **Time**: time zones beyond UTC and fixed offsets, reading `/etc/localtime` and the tz
  database, and formatting with patterns ([STDLIB §5.3](../STDLIB.md#53-time-zinktime-v1)).
- **`Args`**: subcommands (`tool add ...`). Until then, switch on the first positional,
  collect `restValues()` into a `string[..~]`, and give that to a second `Args`'s `parse`.
  The first `Args` rejects options it doesn't know, so the subcommand's options must come
  after a `--` ([STDLIB §6.1](../STDLIB.md#61-arguments-args)).
- **Sorted walks**, an open question: `fs.walk` gives each directory's entries in the file
  system's order ([STDLIB §11](../STDLIB.md#11-open-questions)).
- **`fs.glob`**, built on `fs.walk`, comes with `zink.glob`
  ([STDLIB §9.9](../STDLIB.md#99-glob-patterns-and-regular-expressions-later)).
