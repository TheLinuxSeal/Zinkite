# 12. Calling C

A Zinkite program can call C libraries directly. `extern import` reads a C header, and the
functions in it whose signatures have one clear meaning are called like Zinkite functions,
with their constants as names. An explicit `extern` declaration says what crosses for
everything else, such as a pointer that is really an array. C structs are reached through
**handles**, values that can be passed back to C but never looked into, so no raw pointer
reaches Zinkite code. This chapter covers both kinds of declaration, what may cross, handles,
`errno`, output, what isn't checked, and a walk through `examples/quarry`, a game drawn with
OpenGL through SDL2. The reasons are in
[DESIGN.md §12.6](../DESIGN.md#126-calling-c-decided).

## Importing a header: `extern import`

`extern import` names a header as `#include <...>` would, without the angle brackets or
quotes. Its functions are then called as they are:

```java
extern import stdio.h;
extern import math.h;
extern import string.h;
extern import stdlib.h;

void main() {
    println("from Zinkite");
    puts("from C");
    println("sqrt ${sqrt(2.0)} pow ${pow(2.0, 10.0)} floor ${floor(-2.5)}");
    println("strlen ${strlen("zinkite")} atoi ${atoi("-42")} strcmp ${strcmp("a", "b") < 0}");
    println("EOF ${EOF} SEEK_SET ${SEEK_SET} SEEK_END ${SEEK_END} BUFSIZ ${BUFSIZ}");
}
```

```output
from Zinkite
from C
sqrt 1.4142135623730951 pow 1024.0 floor -3.0
strlen 7 atoi -42 strcmp true
EOF -1 SEEK_SET 0 SEEK_END 2 BUFSIZ 8192
```

A function is called as it is when every parameter and its result have one clear meaning:

| C | Zinkite |
|---|---|
| `int`, `unsigned`, `long`, `size_t`, `int64_t` and the other integers | the integer of the same size and signedness: `i32`, `u32`, `i64`, `u64`, `i64` |
| `char`, `unsigned char` | `i8`, `u8` |
| `float`, `double` | `f32`, `f64` |
| `_Bool` | `bool` |
| an enum, named or a typedef of an unnamed one | `i32` |
| `const char *`, as a parameter | `string`, copied with a NUL onto the scratch stack for the call |
| a pointer to a C struct, such as `FILE *` | a [handle](#handles), `FILE` |
| `void`, as a result | nothing |

- **Typedefs are followed** to what they name, so `size_t` is `u64` and `uint8_t` is `u8`.
- **A typedef of an unnamed enum**, as `typedef enum { ... } SDL_bool;`, is an enum too. SDL
  declares most of its types this way.
- **The generated C includes the header as written**, so the C compiler checks every call
  against the real prototype.
- **Headers are read once per set of them, then cached** (see [Reading headers](#reading-headers-clang-and-the-cache)).
- **A header that can't be found** is an error at the `extern import`: "can't read the C
  headers: 'nosuch/header.h' file not found".

## Constants: macros and enum constants

A header's integer macros, such as `EOF` and `SEEK_SET`, and its enum constants are names
in Zinkite:

```java
extern import dirent.h;
extern import sys/socket.h;
extern import unistd.h;

void main() {
    println("DT_DIR ${DT_DIR} DT_REG ${DT_REG} SOCK_STREAM ${SOCK_STREAM} STDOUT_FILENO ${STDOUT_FILENO}");
    i64 cores = sysconf(_SC_NPROCESSORS_ONLN);
    println("some cores: ${cores > 0}, a pid: ${getpid() > 0}");
}
```

```output
DT_DIR 4 DT_REG 8 SOCK_STREAM 1 STDOUT_FILENO 1
some cores: true, a pid: true
```

- **Only integer macros are names.** A float macro such as `M_PI` is "unknown name
  `M_PI`"; Zinkite's own `math.PI` is the one to use.
- **A function-like macro is not a name.** `SDL_WINDOWPOS_CENTERED` expands to a call of
  another macro, so `examples/quarry` writes its value, `0x2FFF0000`.
- **`extern import` takes no defines.** A function a header declares only under a macro,
  such as OpenGL 1.5's `glBindBuffer` in `GL/gl.h` under `GL_GLEXT_PROTOTYPES`, needs an
  explicit declaration.

## Linking: `extern link`

`extern link name;` links the program with `-lname`:

```java
extern import SDL2/SDL.h;
extern link SDL2;
```

- **It reaches the C compiler through the generated C**, so `zink run` and `zink build`
  both link it.
- **The math library is linked into every program**, so `math.h` needs no `extern link m;`,
  though writing it does no harm.
- **The C library is always there**: `stdio.h`, `string.h`, `stdlib.h`, `unistd.h` and the
  rest need nothing linked.

## What needs an explicit declaration

A C signature can be ambiguous. A pointer could be one value, an array, or something C keeps
after the call; a `char *` result has an owner and a length that C doesn't say. Calling such
a function is an error that says why, and suggests a declaration:

```java
extern import math.h;

void main() {
    i32 e = 0;
    f64 m = frexp(8.0, e);
}
```

```error
example.zn:5:13: error: `frexp` can't be called as its C header declares it: it takes a `int *`, which could be one value, an array or something C keeps
help: declare it with `extern` and Zinkite types that say what crosses, as `extern i64 write(i32 fd, u8[:] data, u64 n);` (DESIGN §12.6)
```

The reasons the compiler gives, with an example of each:

| C function | Why it can't be called as declared |
|---|---|
| `printf` | it takes a variable number of arguments |
| `qsort`, `fread`, `memcpy` | it takes or returns a `void *`, which could be one value, an array or something C keeps |
| `frexp`, `time` | it takes an `int *` or a `time_t *`, for the same reason |
| `getenv`, `strerror` | it returns a `char *`, whose owner and length C doesn't say |
| `strtol` | it takes a `char * *` |
| `div` | it returns a `struct div_t`, a struct by value |

## Explicit declarations

An explicit declaration is `extern`, with the C function's name in parentheses, on a
function with no body. The Zinkite types say what crosses:

```java
extern import math.h;

// frexp and modf write a result through a pointer: a writable view of one element.
extern("frexp") f64 splitFloat(f64 x, i32[:]& exponent);
extern("modf") f64 splitParts(f64 x, f64[:]& whole);

// getenv's result is a char *: here, only whether it is NULL matters.
extern("getenv") u64 envAddress(string name);

void main() {
    i32[.1] e = [0];
    f64 m = splitFloat(48.0, e);
    println("48 = ${m} * 2^${e[0]}");
    f64[.1] whole = [0.0];
    f64 frac = splitParts(3.25, whole);
    println("3.25 = ${whole[0]} + ${frac}");
    println("set: ${envAddress("NO_SUCH_VARIABLE_HERE") != 0}");
}
```

```output
48 = 0.75 * 2^6
3.25 = 3.0 + 0.25
set: false
```

- **`extern("c_name") ret name(params);`** gives the C function `c_name` the Zinkite name
  `name`. With the same name in both, the parentheses can go: `extern i64 write(i32 fd,
  u8[:] data, u64 n);`.
- **It takes precedence over the header's declaration** of the same function, so a
  function can be declared under its own name even with its header imported. The C side gets
  a prototype of its own, bound to the C symbol, which can't clash with the header's.
- **At the top of a script file** it is a function like the others. **In a class** it is
  `static extern ...`; without `static`, it is "a C function is `static`".
- **Names that start with `zr_`** are the runtime's, which only the standard library calls:
  "`zr_` functions are the runtime's, which only the standard library calls".

### What may cross

| Zinkite parameter | What C gets |
|---|---|
| a number or `bool` | the value |
| `string` | the text, copied with a NUL onto the scratch stack for the call: a `const char *` |
| `u8[:]`, `i32[:]`, `f64[:]`, any slice of numbers | a pointer to the first element, and nothing else: the length is a parameter of its own |
| `u8[:]&`, any writable view of numbers | the same pointer, which C may write through |
| a handle | the C pointer |

| Zinkite result | |
|---|---|
| a number or `bool` | the value |
| a handle | the C pointer |
| `void` | nothing |

```java
extern import stdio.h;

// fwrite and fread take a `void *`, which the header can't say is an array of doubles.
extern("fwrite") u64 writeDoubles(f64[:] data, u64 size, u64 count, FILE f);
extern("fread") u64 readDoubles(f64[:]& data, u64 size, u64 count, FILE f);

void main() {
    string path = "/tmp/zink-docs-12-doubles.bin";
    f64[..>] xs = [1.5, 2.25, -3.0];
    FILE out = fopen(path, "wb");
    u64 wrote = writeDoubles(xs, 8, (u64) xs.len, out);
    fclose(out);
    f64[..>] back = [0.0, 0.0, 0.0, 0.0];
    FILE input = fopen(path, "rb");
    u64 got = readDoubles(back, 8, (u64) back.len, input);
    fclose(input);
    remove(path);
    println("wrote ${wrote}, read ${got}: ${back}");
}
```

```output
wrote 3, read 3: [1.5, 2.25, -3.0, 0.0]
```

- **Any array of numbers passes as a slice**, as `xs`, a growable array, does above. Its
  elements are contiguous, so C sees an ordinary C array.
- **A writable view lets C fill an array the program made**, which must already be as long
  as what C writes: `back` has four elements before `fread` writes three.
- **`string` and `u8[:]` differ here**, though they are the same type elsewhere: a parameter
  written `string` is copied with a NUL, for C that reads text up to one, and one written
  `u8[:]` passes the bytes' pointer with no NUL, for C that is told the length.
- **A `void *` result can only be declared as an integer**, a `u64`, as `envAddress` does.
  The program can compare it with 0 or pass it back to C through a `u64` parameter, and
  nothing more.
- **A `char **` can't cross.** An array of pointers to text is nothing Zinkite can make, so
  a function such as OpenGL's `glShaderSource` can't be called.
- **What can't cross is an error at the declaration**: an array on a data stack, a class, a
  reference, a capped array, a slice of anything but numbers, or a `func` value is "`i64[..>]`
  can't cross into C", with the help listing what can; a result of any other type is "a C
  function can't return string".

## Handles

A C struct that a header uses through pointers, such as `FILE` or `SDL_Window`, is a type of
its own in Zinkite: a handle. It is a value that can be stored, copied, compared, set to and
compared with `null`, and passed back to C:

```java
extern import stdio.h;

class Log {
    FILE out;
    i64 lines;
}

void main() {
    string path = "/tmp/zink-docs-12-handles.txt";
    FILE f = fopen(path, "w");
    if (f == null) {
        println("can't create ${path}: ${status.message(extern.errno())}");
        return;
    }
    Log log = new Log(f, 0);
    fputs("one\n", log.out);
    fputs("two\n", log.out);
    FILE same = log.out;
    println("same handle: ${same == f}");
    fclose(f);
    FILE g = fopen(path, "r");
    i64 count = 0;
    i32 c = fgetc(g);
    while (c != EOF) {
        if (c == '\n') {
            count++;
        }
        c = fgetc(g);
    }
    fclose(g);
    remove(path);
    FILE none = null;
    println("${count} lines, none is null: ${none == null}");
}
```

```output
same handle: true
2 lines, none is null: true
```

- **The handle's name** is the struct's typedef, as `FILE`, or `struct_name` for a struct
  with none, as `struct box *` gives `struct_box`.
- **A handle is never looked into.** Its fields are "FILE has no field `_fileno`", a cast is
  "casts exist only for numbers, and this is FILE", and `new FILE()` is "`new` constructs
  classes, and FILE isn't one". It can't be printed either.
- **It stays in its thread.** A worker can't capture one, and a queue can't hold one or a
  class with one: "a worker can't take `f` with it: `FILE` is a C pointer"
  ([Threads](11-threads.md)).
- **It isn't a `File`.** Zinkite's runtime doesn't track it; C's own rules apply, such as
  `exit` flushing open `FILE` streams.
- **Nothing checks it on the C side**: a handle used after `fclose` is C's undefined
  behavior, as it would be in C.

## `errno`: `extern.errno()`

A C function never fails by itself: it returns what C returns, and its status is always 0.
`extern.errno()` reads C's `errno` as a `u8`, and since Zinkite's status codes follow
`errno`'s numbers ([Named codes](07-errors.md#named-codes)), it can be returned as a status
as it is:

```java
extern import stdio.h;

FILE openOrFail(string path, string mode) {
    FILE f = fopen(path, mode);
    if (f == null) {
        return #(extern.errno()) f;
    }
    return f;
}

void main() {
    FILE f @err = openOrFail("/nonexistent/dir/file", "r");
    println("null ${f == null}, ${status.name(err)}: ${status.message(err)}");
    u8 e = @puts("puts succeeds as C says, and its status is always 0");
    println("${e}");
}
```

```output
null true, NOT_FOUND: No such file or directory
puts succeeds as C says, and its status is always 0
0
```

- **Read it right after the call** that set it. Anything in between, a `println` included,
  may change `errno`.
- **`#` needs parentheses around it**: `return #(extern.errno()) f;`. Written
  `#extern.errno()`, it is "expected a status after `#`: a number, a named code or `(...)`".
- **It is `errno` as a status**: the same number, except that an `EPERM` (1) reads as
  `PERMISSION` (13), since 1 is `FAILED` ([Errors and statuses](07-errors.md)).

## Output and C's stdout

Zinkite's output goes through C's `stdout`, so text printed by Zinkite and by C code it calls
comes out in the order it was written, on a terminal and through pipes alike, as the first
example showed. A C function that writes to the file descriptor directly bypasses that
buffer:

```java
extern i64 write(i32 fd, u8[:] data, u64 n);

void main() {
    println("first, through stdout");
    u8[..>] msg = [];
    msg.append("second, through write(2)\n");
    i64 n = write(1, msg, (u64) msg.len);
    println("third, through stdout: ${n}");
}
```

```output
second, through write(2)
first, through stdout
third, through stdout: 25
```

The first line was still in `stdout`'s buffer when `write` ran. `stdout.flush()` writes out
what is pending:

```java
extern i64 write(i32 fd, u8[:] data, u64 n);

void main() {
    println("first, through stdout");
    stdout.flush();
    u8[..>] msg = [];
    msg.append("second, through write(2)\n");
    i64 n = write(1, msg, (u64) msg.len);
    println("third, through stdout: ${n}");
}
```

```output
first, through stdout
second, through write(2)
third, through stdout: 25
```

- **stdout is buffered by lines on a terminal and in 64 KB blocks otherwise**, which is
  why the order above changed: the test that runs these examples reads through a pipe.
- **stderr is unbuffered**, and Zinkite flushes stdout before it writes to stderr.
- **This example needs no `extern import`**: an explicit declaration is enough to call a
  C function, and clang isn't needed to read anything.

## Zinkite names win

A C function with the name of a Zinkite function or built-in is reached through an explicit
declaration under another name. `abs` is Zinkite's, which works for every number type and
fails with `RANGE` on `i64.MIN`:

```java
extern import stdlib.h;

extern("abs") i32 cAbs(i32 n);

void main() {
    println("${abs(-2.5)} ${abs(-7)} ${cAbs(-7)}");
    i64 m = i64.MIN;
    i64 a @err = abs(m);
    println("${a} ${status.name(err)}");
}
```

```output
2.5 7 7
0 RANGE
```

## Reading headers: clang and the cache

- **Reading a header needs clang**, which lists what it declares: `$CC` when that is a
  clang, or `clang` on the path. Without one, `extern import` is "can't read the C headers:
  reading C headers needs clang, which can list what a header declares; install clang". The program itself is then
  built with whichever C compiler `zink` finds ([Getting started](01-getting-started.md)).
- **What clang found is cached** in the cache directory, `$XDG_CACHE_HOME/zink` or
  `~/.cache/zink`, keyed by the headers' names, the clang binary and the version of the
  reader, and kept while every file clang read is unchanged. A cached read checks those
  files' sizes and times and starts no process. `zink cache clean` empties it.
- **The headers are found where clang finds them**: the system's include directories, and
  those in `C_INCLUDE_PATH`.
- **The documentation's tests skip** an example with `extern import` on a machine without
  clang.

## What isn't checked

The C side isn't checked. Getting a declaration wrong can break the guarantees the rest of
the language gives:

- **A declaration that gets a pointer's meaning wrong**, such as a writable view shorter than
  what C writes into it, lets C write past the array.
- **C that keeps a pointer past the call**, such as a buffer given to a library that reads
  it later, can read memory that has since been freed or reused.
- **A handle used after C freed what it points to** is undefined, as in C.
- **A C function that crashes, or calls `exit` or `abort`, ends the program**, and nothing in
  Zinkite can catch it, `@@` included. `exit` from `stdlib.h` is callable as it is; it
  flushes stdout and the program's open Files, as returning from `main` does.

So explicit declarations belong in small, careful wrappers: a function of the program's
own that takes Zinkite values, checks them, calls C, and fails with a status. The graphics
bindings designed in [STDLIB.md §10.6](../STDLIB.md#106-graphics-vendorglfw-vendorsdl3-vendorgl-vendorvulkan)
are such wrappers, generated for whole libraries.

## A walk through `examples/quarry`

`examples/quarry/quarry.zn` is a voxel sandbox in a window, about 1,800 lines, drawn with
OpenGL through SDL2. Its [README](../examples/quarry/README.md) says how to run it, and
[NOTES.md §6](../examples/NOTES.md#6-quarry-a-game-through-cs-headers) what writing it
showed. The code below needs SDL2's and OpenGL's development files, so it isn't checked by
the documentation's tests; the compiler's own tests build quarry where they are installed.

### The headers

```java no-check
extern import SDL2/SDL.h;
extern import GL/gl.h;
extern link SDL2;
extern link GL;
```

41 SDL and OpenGL functions take only numbers, text and handles, and are called as they
are, with constants such as `GL_DEPTH_TEST` and `SDL_SCANCODE_W`. 11 take pointers, and each
has a declaration that says what its pointer means:

```java no-check
extern("SDL_GL_CreateContext") u64 createContext(SDL_Window w);    // an SDL_GLContext, a void *
extern("SDL_PollEvent") i32 pollEvent(u32[:]& event);              // fills an SDL_Event, 56 bytes
extern("SDL_GL_GetDrawableSize") void drawableSize(SDL_Window w, i32[:]& width, i32[:]& height);
extern("glGenBuffers") void genBuffers(i32 n, u32[:]& ids);
extern("glBindBuffer") void bindBuffer(u32 target, u32 id);        // GL 1.5, so not in gl.h
extern("glBufferData") void bufferData(u32 target, i64 bytes, f32[:] data, u32 usage);
extern("glInterleavedArrays") void interleavedArrays(u32 format, i32 stride, u64 offset);
extern("glGenTextures") void genTextures(i32 n, u32[:]& ids);
extern("glTexImage2D") void texImage2D(u32 target, i32 level, i32 internal, i32 width,
                                       i32 height, i32 border, u32 format, u32 type,
                                       u8[:] pixels);
extern("glReadPixels") void readPixels(i32 x, i32 y, i32 width, i32 height, u32 format,
                                       u32 type, u8[:]& pixels);
extern("glFogfv") void fogColor(u32 name, f32[:] rgba);
```

- **Slices of numbers** carry the vertex data, the textures' pixels and the fog's color.
- **Writable views** take what C writes: buffer and texture ids, a screenshot's pixels, and
  the event.
- **`SDL_GL_CreateContext` returns a `void *`**, declared as a `u64`, since the program only
  checks that it isn't 0.
- **`glInterleavedArrays` takes a pointer** that, with a vertex buffer bound, is an offset
  into the buffer, so it is declared as a `u64` and given 0.

### The window

```java no-check
if (SDL_Init(SDL_INIT_VIDEO) != 0) {
    errprintln("quarry: SDL can't start its video; with no screen, set SDL_VIDEODRIVER=offscreen");
    return 1;
}
SDL_GL_SetAttribute(SDL_GL_DEPTH_SIZE, 24);
SDL_GL_SetAttribute(SDL_GL_DOUBLEBUFFER, 1);
SDL_Window window = SDL_CreateWindow("quarry", 0x2FFF0000, 0x2FFF0000, (i32) width,
                                     (i32) height, SDL_WINDOW_OPENGL | SDL_WINDOW_RESIZABLE);
if (window == null) {
    errprintln("quarry: SDL can't open a window");
    SDL_Quit();
    return 1;
}
if (createContext(window) == 0) {
    errprintln("quarry: SDL can't make an OpenGL context");
    SDL_Quit();
    return 1;
}
```

- **`SDL_CreateWindow` takes its title as a `const char *`**, so a Zinkite `string` is copied
  with a NUL for it, and gives an `SDL_Window` handle, compared with `null`.
- **`SDL_GLattr` and `SDL_bool` are typedefs of unnamed enums**, so `SDL_GL_SetAttribute`
  and `SDL_SetRelativeMouseMode(SDL_TRUE)` are called as they are.
- **`0x2FFF0000` is `SDL_WINDOWPOS_CENTERED`**, a function-like macro, written as its value.

### Events, read from an array of `u32` by offset

An `SDL_Event` is a C union of 56 bytes. quarry keeps one as 16 `u32`s, which
`SDL_PollEvent` fills through a writable view, and reads its fields by their offsets:

```java no-check
u32[.16] ev = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
...
while (pollEvent(ev) != 0) {
    u32 type = ev[0];
    if (type == SDL_QUIT) {
        running = false;
    } else if (type == SDL_KEYDOWN || type == SDL_KEYUP) {
        i32 key = (i32%) ev[4];                 // keysym.scancode, at byte 16
        bool repeat = ((ev[3] >> 8) & 0xFF) != 0;
        if (key >= 0 && key < 512) { in.down[key] = type == SDL_KEYDOWN; }
        ...
    } else if (type == SDL_MOUSEMOTION && in.captured) {
        player.yaw += (f64) (i32%) ev[7] * 0.0025;      // xrel and yrel, bytes 28 and 32
        player.pitch = clamp(player.pitch + (f64) (i32%) ev[8] * 0.0025, -1.55, 1.55);
    }
    ...
}
```

- **`(i32%) ev[4]`** reinterprets the `u32` as the signed field it is in C, with the
  wrapping cast.
- **The offsets were checked against `offsetof` in C, by hand.** Nothing checks them in
  Zinkite, which is what makes reading a struct this way fragile; a way to give a class C's
  layout is open ([DESIGN §17](../DESIGN.md#17-open-decisions)).

### Vertex buffers

Each chunk of the world has two vertex buffers, made once, filled each time the chunk
changes, and drawn each frame. The ids come from `glGenBuffers`, which writes them into a
growable array:

```java no-check
u32[..>] ids = [];
for (i64 i = 0; i < count * 2; i++) { ids +> 0; }
genBuffers((i32) (count * 2), ids);
```

A vertex is eight `f32`s, in the order `GL_T2F_C3F_V3F` wants, appended to an `f32[..>]`:

```java no-check
void vertex(f32[..>]& out, i64 tile, i64 cu, i64 cv, f64 shade, f64 x, f64 y, f64 z) {
    ...
    out +> (f32) u +> (f32) v +> s +> s +> s +> (f32) x +> (f32) y +> (f32) z;
}

void upload(u32 buffer, f32[..>]& data) {
    bindBuffer(GL_ARRAY_BUFFER, buffer);
    bufferData(GL_ARRAY_BUFFER, data.len * 4, data, GL_STATIC_DRAW);
}
```

`bufferData` passes the array's pointer, so OpenGL copies the floats straight from where
Zinkite built them. Drawing binds each buffer and points OpenGL at offset 0 in it:

```java no-check
bindBuffer(GL_ARRAY_BUFFER, pass == 0 ? c.solidBuffer : c.waterBuffer);
interleavedArrays(GL_T2F_C3F_V3F, 0, 0);
glDrawArrays(GL_QUADS, 0, count);
```

### Statuses around C

quarry's own code uses statuses as any program does. Loading a saved world reads its
numbers and runs of blocks without checking each step, and a short or damaged file fails
wherever it first reads past the end:

```java no-check
u8 bad = @@world.load(data, player);
if (bad) {
    errprintln("quarry: ${worldFile} isn't a world this program wrote (${status.message(bad)})");
    return 1;
}
```

A screenshot is `glReadPixels` into a writable view of a `u8[..>]`, then `File.writeAll`,
whose failure `@@screenshot(view, file)` reports.

### What it ran into

- **Shaders need `glShaderSource`**, whose source is a `char **`, so the drawing is OpenGL
  1.5's fixed pipeline.
- **`extern import` takes no defines**, so each GL 1.5 function is declared by hand.
- **Function-like macros are not names**, so `SDL_WINDOWPOS_CENTERED` is a number.
- **A struct read by offsets** works, but is checked only by hand.

It runs as fast as C would: at `-O2`, generating 5 million blocks and building 256 chunk
meshes take about 0.3 s, and a frame costs about 0.1 ms of the CPU.

## Not yet

These are open in [DESIGN §17](../DESIGN.md#17-open-decisions), and collected in
[What isn't there yet](15-not-yet.md):

- passing function pointers to C, for callbacks such as `qsort`'s comparator;
- structs by value, as arguments and results;
- C strings that C returns, as `getenv`'s;
- C's global variables;
- a C layout for classes, so a struct such as `SDL_Event` could be a class and a slice of
  vertices could be uploaded as it is;
- defines for `extern import`, and function-like macros as names.
