# Zinkite

A small, statically typed language for Linux command-line tools and embedded
scripts. It has no heap, no garbage collector and no reference counting, and
it checks memory safety at compile time, without lifetime annotations. The
compiler, `zink`, turns a program into C and builds it with clang or gcc.

Source files end in `.zn`. The name is a spelling of zincite, the zinc oxide
mineral: Rust is iron oxide, and zinc is what keeps iron from rusting.

> Zinkite is early and still changing. The design documents are drafts, and
> the compiler doesn't cover all of them yet (see [Status](#status)).

## A taste

```java
#!/usr/bin/env zink
import zink.cli.*;
import zink.io.*;

class Hit {
    string text;    // a view into the file's bytes: nothing is copied
    i64 line;
}

u8 main(string[.] args) {
    Args cli = new Args("grepish", "Print the lines of a file that contain a word.");
    cli.flag('n', "number", "show line numbers");
    cli.positional("word", "what to look for");
    cli.positional("file", "the file to search");
    u8 code @stop = cli.parseOrStop(args);
    if (stop) { return code; }
    string word = cli.get("word", "");

    // Read straight into text's own storage, freed at the end of main.
    u8[.] text @err = File.readAll(cli.get("file", ""));
    if (err != 0) {
        errprintln("grepish: ${status.message(err)}");
        return 2;
    }

    Hit[..>] hits = [];
    i64 n = 0;
    for (string line : text.lines()) {          // lazy: no array of lines is built
        n++;
        if (line.contains(word)) { hits +> new Hit(line, n); }
    }
    for (Hit& h : hits) {
        if (cli.has("number")) { print("${h.line}: "); }
        println(h.text);
    }
    return hits.len > 0 ? 0 : 1;
}
```

```
$ zink grepish.zn -n the notes.txt
1: the quick fox
3: the lazy dog
```

## The ideas

- **No heap.** Values live on the native stack, in static data, or on *data
  stacks*: large address-space reservations with a bump pointer, assigned by the
  compiler. When a scope ends, its memory goes back with one store. Growing an
  array never moves it, so references into it stay valid.
- **Data stored inline.** A record with text and nested arrays is one
  contiguous run of bytes, with no pointers inside it.
- **References checked at compile time.** Every reference has a root that the
  compiler infers, and it can't outlive it. Removing from an array while a
  reference into it may still be used is an error:
  ```
  safe.zn:7:5: error: `clear` removes from `items` while `first` may still refer into it
      items.clear();
      ^^^^^^^^^^^^^
      help: `first`, declared on line 6, is used after this; use it before the removal, or bind it again after (REFERENCES §5.2)
  ```
- **Statuses instead of exceptions.** Every call returns a `u8` status beside
  its value: `return #status.NOT_FOUND x;` sets it, `x @err = f();` reads it.
  The codes follow Linux's `errno` numbers. Every failure is one, a missing
  file and an index out of range alike, and `@` can read any of them:
  `v @err = a[i] * 2;`. A failure nothing reads stops the program at its line:
  ```
  tool.zn:12: `open` failed with NOT_FOUND, and nothing read its status
  ```
  `@@err` also catches one that nothing read deeper inside the call, and
  `@@@trail` says where it went, so a server can fail one request and carry on.
  Nothing else stops a program, and there is no `exit`: it ends when `main`
  returns.
- **Arrays that say what they allow.** `T[..>]` only grows at the end,
  `T[~.~]` changes at both ends, and `T[~]` has a cursor, as an editor's buffer
  does. Each gets the layout that suits it, and the compiler checks that the
  code only does what the type allows.
- **Familiar syntax, no inheritance.** Classes with methods, constructors and
  `static` members; interfaces for dynamic dispatch; generics, compiled once
  per type; lambdas; `defer` instead of destructors.
- **Calling C.** `extern import stdio.h;` reads a header with clang, and its
  functions are called like any other.

The memory model and the reference rules follow those of the Goose language.
`DESIGN.md` explains each decision and the alternatives that were rejected.

## Status

The compiler parses and type-checks the whole grammar, and runs the reference
checks for lifetimes, removals, variable-size values and links (rules S, R, V and
L of `REFERENCES.md`). It generates C for:
- classes, generics, lambdas and interfaces;
- records stored inline, with their text and nested arrays in one block;
- every kind of array, hash maps, sets and pools;
- statuses;
- threads that share no memory, with queues between them;
- calling C.

The standard library's first tier is written, in Zinkite on a small C layer, and
built into the compiler: files, directories and paths (`zink.io`), running programs
(`zink.os`), time and dates (`zink.time`), argument parsing and colors (`zink.cli`),
random numbers, hashing, hex and base64, CSV, and JSON, read as events or into a tree
and written back. Only `hash.xxh3` is missing from it. TOML and YAML come next, into
the same tree.

All ten programs in `examples/` run. The two largest are:
- [`examples/quarry`](examples/quarry), a voxel sandbox in a window, drawn with OpenGL
  through SDL2's C headers;
- [`examples/lisp.zn`](examples/lisp.zn), a Scheme-like interpreter with a garbage
  collector.

`compiler/README.md` lists what's done and what isn't, in detail.

## Building

You need:
- 64-bit Linux, since data stacks rely on virtual memory;
- Rust 1.88 or later;
- a C compiler. clang is preferred, and is needed for `extern import`; gcc works
  for everything else.

```
cd compiler
cargo build --release
cargo test
```

`cargo install --path compiler` puts `zink` in `~/.cargo/bin`.

### With Nix

The flake's `zink` brings its own clang, which it uses when the path has no clang
and `$CC` isn't set. `nix run github:TheLinuxSeal/Zinkite -- FILE` runs a program
without installing anything. To install it, add the flake to a system or Home
Manager flake:

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

With Home Manager, put the same package in `home.packages`. The flake also has an
overlay, `zink.overlays.default`, which adds `pkgs.zink`. `nix flake update zink`
picks up a newer compiler. The nixpkgs it follows needs Rust 1.88 or later, as in
NixOS 26.05. The tests run in `nix flake check`, not when the package is built.

## Using it

```
zink run [-O] FILE [args]   compile into the cache if needed, then run it
zink FILE [args]            the same, for #!/usr/bin/env zink
zink build FILE -o OUT      build an executable at -O2
zink check FILE...          type-check without building
zink c FILE                 print the generated C
```

`zink run` caches each built executable in `~/.cache/zink`, so running a script
again skips the C compiler. That is `$CC` if set, then clang, then gcc.

## Documents

| File | What it holds |
|---|---|
| [`DESIGN.md`](DESIGN.md) | the language: every decision, its status and its reasons |
| [`GRAMMAR.md`](GRAMMAR.md) | the grammar |
| [`REFERENCES.md`](REFERENCES.md) | the reference rules the compiler checks |
| [`STDLIB.md`](STDLIB.md) | the standard library |
| [`compiler/README.md`](compiler/README.md) | the compiler: what it covers, how it's laid out |
| [`examples/`](examples) | example programs, and `NOTES.md` on what writing them showed |
| [`bench/`](bench) | C prototypes of the layouts, with the measurements that chose them |
| `DESIGN-refcount.md`, `GRAMMAR-refcount.md` | an earlier design based on reference counting, kept for reference |

## License

[Apache License 2.0](LICENSE).
