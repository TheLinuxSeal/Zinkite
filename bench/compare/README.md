# Zinkite against Goose, C++ and Rust

26 benchmarks, each written in Zinkite, Goose, C++ and Rust, and a harness that builds
every implementation at three sizes, checks they all print the same output, and
records the best wall time and the peak memory of each. The results are in
[`results.md`](results.md).

```
python3 bench/compare/run.py                          # everything, about an hour
python3 bench/compare/run.py --only sum,tree          # a subset; results.md keeps the rest
python3 bench/compare/run.py --sizes small --reps 1   # a quick pass
python3 bench/compare/run.py --langs zinkite,cpp      # some languages
python3 bench/compare/run.py --report-only            # rewrite results.md from results.json
```

It needs `clang` and `clang++` (or `--cc` and `--cxx`), `rustc`, a built `zink` (the newer
of `compiler/target/release` and `debug`), and a Goose compiler, given with `--goose` or
`$GOOSE`. Goose isn't packaged; it builds from source in a few minutes:

```
git clone https://github.com/aardappel/goose && cd goose
cmake -B build -DCMAKE_BUILD_TYPE=Release && cmake --build build -j
export GOOSE=$PWD/build/goose
```

## How it measures

- Each implementation is a whole program, timed from start to exit: generating its
  input, building its structures, the work, and tearing them down. There are no input
  files: every program makes its data with the same xorshift64 generator from the same
  seed, so all four languages work on identical data.
- The size is the number on the line marked `// BENCH_N`, rewritten in a copy of the
  source for Zinkite, Goose and Rust, and passed as `-DBENCH_N` to C++.
- Every implementation of a benchmark must print the same lines. A time whose output
  differs from the others' is marked ✗ and left out of the comparisons.
- C++ and the C that Zinkite and Goose generate are compiled by the same clang at -O2.
  Rust is `rustc -O -C codegen-units=1`, also LLVM, with its default bounds checks.
- Each time is the best of three runs after one discarded warm-up. Memory is the peak
  resident set, read by `launch.c`, which runs each program so that its count isn't
  inflated by the Python process that started it.
- C++ is measured at two tiers where Goose's suite has both: *idiomatic*, what a C++
  programmer writes first (`unique_ptr`, `vector<string>`, `unordered_map`), and
  *expert*, the hand-tuned version (arenas with indices, `string_view`, open
  addressing, CSR).

## Where the benchmarks come from

- **The first sixteen** (`sum` to `blur`) are Goose's own benchmark suite, from
  [github.com/aardappel/goose](https://github.com/aardappel/goose) at commit `2322149`,
  `bench/`. Their Goose, C++ and Rust sources (`goose/`, `cpp/`, `rust/`, including
  `rng.goose`, `bench.h` and `bench.rs`) are copied unchanged, and their sizes, rows and
  descriptions in `benchmarks.py` are adapted from that suite's `run_bench.py`. They are
  under the Apache License 2.0, in [`LICENSE-goose`](LICENSE-goose). The Zinkite
  implementation of each is new.
- **The other ten** (`fib` to `base64`) were written for this suite in all four
  languages. Several Goose versions are adapted from Goose's programs for the same
  problems, as each file's header says; those are under the same license.
- The Zinkite programs follow the design of the Goose ones where Zinkite has the same
  feature, and say in their header where they differ: Zinkite has no compact
  `T&<i32>` links in its code generator yet, no enums with payloads, no varints and no
  `return from`, so those programs use references, indices, a variable-size class with
  a kind field, `i32` fields and `@@` instead.

## Things to know when reading the numbers

- **Zinkite checks every integer operation and every index**, and stops the program
  with a status when one fails (DESIGN §7.6, §9.2). Goose checks indexes but proves
  most checks away, and C++ checks neither. The checks keep clang from vectorizing
  reductions (`sum`) and from inlining small functions (`spectralnorm`); that cost is
  measured here, not hidden.
- **base64** uses each language's standard library where it has one: Zinkite's
  `zink.encoding` and Goose's `std` codec, which is vectorized. C++ and Rust have none
  in their standard libraries, so they use a hand-written table codec.
- **matmul**'s Goose version keeps `simd fn` from Goose's own program, which may use
  wider vector instructions than the baseline x86-64 build the others get.
- **mandelbrot** and **spectralnorm** are the plain row-by-row loops in all four
  languages, not the Benchmarks Game's vectorized versions.

## What the latest run found (2026-11-22, second run)

Geometric means over the 26 benchmarks, at the large size, of each language's fastest
implementation's time over Zinkite's (above 1 means Zinkite is faster), against the first
run the same day:

| | first run | second run |
|---|---:|---:|
| Goose | 0.69 | 0.81 |
| idiomatic C++ | 1.30 | 1.53 |
| expert C++ (the sixteen with an expert row) | 1.02 | 1.05 |
| Rust | 0.82 | 0.97 |
| peak memory: Goose / C++ / Rust | 0.92 / 1.32 / 1.05 | 0.97 / 1.39 / 1.11 |

Zinkite is now faster than Goose on 5 benchmarks (calc, respond, nbody, mandelbrot,
nsieve), within 10% on 10 more, and slower on 11. What changed between the runs:

| change | where it showed |
|---|---|
| base64's loops moved into the runtime, vectorized with AVX2 or AVX-512 where the CPU has them | base64 3979 → 300 ms |
| small functions emitted `static inline`, so their checks don't keep clang from inlining them | spectralnorm 7398 → 2421 ms; calc 700 → 652 |
| fixed arrays, `T[.k]`, keep no length: it is the constant `k` | scene's 3×3 matrices: 1431 → 1009 ms |
| self-relative links, `T&<i32>`: 4-byte offsets in place of 8-byte references | sexp 2475 → 2105 ms, 750 → 511 MB; tree 12-byte nodes, 386 MB; bintrees 26 MB |
| enums stored in the narrowest integer, a byte for up to 256 constants | records 252 → 206 MB |

The gaps that remain, largest first:

| benchmark | Zinkite / Goose | cause |
|---|---:|---|
| levenshtein | 2.5× | every index and `+ 1` is checked, and none of the checks are removed; without them it would be 1.1× |
| words | 2.0× | the built-in hashmap's layout: a lookup reads its index, then its entry, then the key, three cache misses where Goose's table makes two. DESIGN §4.14 chose this layout knowing it finds keys more slowly in maps too big for the cache |
| spectralnorm | 1.9× | the overflow checks in `(i + j) * (i + j + 1) / 2`: without them it takes 1440 ms, against Goose's 1277 |
| fannkuch, fib | 1.6–1.7× | checked indexing and arithmetic; in fib, the check on `fib(n - 1) + fib(n - 2)` stops clang turning one call into a loop, which no form of the check avoids |
| interp | 1.6× | not yet explained; removing the checks gains 3% |
| bintrees, tree, records | 1.4× | tree's and bintrees' nodes are now Goose's size, and checks are 6–14% of the rest; records has no variants with payloads, so every event carries every variant's fields |
| sum | 1.24× | the overflow check on `total +=` keeps the loop from vectorizing |
| scene, sexp | 1.1× | scene's children are 8-byte references in an array, since links are only fields; sexp's nodes carry every variant's fields |

Most of the rest would come from removing the checks the compiler can prove can't fail,
and from loop versioning: checking once before a loop that its arithmetic can't overflow.

Compiler bugs the ports work around:
- `pushAll` on arrays passes the checker but isn't in the code generator.
- A for-each over an array of inline strings (`u8[.*][..>]`) by reference generates C that
  doesn't compile, and by `string` gives the element the wrong C type.
- A function returning an array of variable-size records generates C that doesn't compile.
- Copying into a writable view (`a[:] = b`) generates C that doesn't compile.
- A function that stores a reference parameter into a static array gets a false
  lifetime error, reported against `zink/io/File.zn`.
- `long` as a name says "`i64` is a type, not a value" before it says `long` is reserved.
- A constructor that fails doesn't reset the fields it set before failing (DESIGN §6.2).

## Files

| | |
|---|---|
| `run.py` | the harness |
| `benchmarks.py` | the manifest: each benchmark's sizes and implementations |
| `launch.c` | runs one program and reports its time and peak memory |
| `zinkite/`, `goose/`, `cpp/`, `rust/` | the implementations |
| `results.md`, `results.json` | the last run's results |
| `gen/` | build products (not committed) |
