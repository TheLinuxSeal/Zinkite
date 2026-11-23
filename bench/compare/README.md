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
  feature, and say in their header where they differ: Zinkite has links (`T&<i32>`) only
  as fields, no enums with payloads, no varints and no `return from`, so those programs
  use references in arrays, a variable-size class with a kind field, `i32` fields and
  `@@` instead.

## Things to know when reading the numbers

- **Zinkite checks every index**, and stops the program with a status when one fails
  (DESIGN §7.6). Goose checks indexes but proves most checks away, and C++ checks
  none. None of the four checks integer arithmetic now: Goose's and Rust's release
  builds wrap, C++ doesn't check, and Zinkite's `+` has wrapped since 2026-11-22, with
  `+#` as its checked add (DESIGN §9.2). The first runs below, before that, checked
  every `+`.
- **base64** uses each language's standard library where it has one: Zinkite's
  `zink.encoding` and Goose's `std` codec, which is vectorized. C++ and Rust have none
  in their standard libraries, so they use a hand-written table codec.
- **matmul**'s kernel is a `simd` function in Goose and in Zinkite (DESIGN §12.8), so
  those two run it with AVX-512 on this machine; C++ and Rust are built for baseline
  x86-64, as everything else is.
- **mandelbrot** and **spectralnorm** are the plain row-by-row loops in all four
  languages, not the Benchmarks Game's vectorized versions.

## What the latest run found (2026-11-22)

Geometric means over the 26 benchmarks, at the large size, of each language's fastest
implementation's time over Zinkite's (above 1 means Zinkite is faster), over the four
runs of the day:
1. the first run;
2. after the compiler changes in the table below;
3. after the ports dropped their workarounds for compiler bugs, then fixed (the seven
   benchmarks whose ports changed, re-run in all four languages);
4. after `+`, `-` and `*` began to wrap (DESIGN §9.2), every benchmark re-run.

| | 1 | 2 | 3 | 4 |
|---|---:|---:|---:|---:|
| Goose | 0.69 | 0.81 | 0.83 | 0.92 |
| idiomatic C++ | 1.30 | 1.53 | 1.56 | 1.73 |
| expert C++ (the sixteen with an expert row) | 1.02 | 1.05 | 1.08 | 1.15 |
| Rust | 0.82 | 0.97 | 0.99 | 1.10 |
| peak memory: Goose / C++ / Rust | 0.92 / 1.32 / 1.05 | 0.97 / 1.39 / 1.11 | 0.97 / 1.39 / 1.11 | 0.97 / 1.38 / 1.10 |

Zinkite is now faster than Goose on 8 benchmarks (push, tree, lru, calc, respond, nbody,
nqueen, nsieve), within 10% on 10 more, and slower on 8. What changed between the runs:

| change | where it showed |
|---|---|
| base64's loops moved into the runtime, vectorized with AVX2 or AVX-512 where the CPU has them | base64 3979 → 300 ms |
| small functions emitted `static inline`, so their checks don't keep clang from inlining them | spectralnorm 7398 → 2421 ms; calc 700 → 652 |
| fixed arrays, `T[.k]`, keep no length: it is the constant `k` | scene's 3×3 matrices: 1431 → 1009 ms |
| self-relative links, `T&<i32>`: 4-byte offsets in place of 8-byte references | sexp 2475 → 2105 ms, 750 → 511 MB; tree 12-byte nodes, 386 MB; bintrees 26 MB |
| enums stored in the narrowest integer, a byte for up to 256 constants | records 252 → 206 MB |
| run 3: words counts from its list of words, with keys that are views into that compact block, where it split the text again and kept views into the text | words: open addressing 4118 → 2452 ms, the built-in hashmap 5199 → 4464 |
| run 3: fannkuch's arrays are `i32[.16]` and `perm = perm1` copies one, where a loop copied `[..>]` arrays | fannkuch 3120 → 2898 ms |
| run 4: `+`, `-` and `*` wrap, and `+#`, `-#` and `*#` check, where `+` checked | fib 1793 → 1170 ms; spectralnorm 2421 → 1318; tree 2071 → 1507; bintrees 586 → 430; sum 1014 → 818; levenshtein 3146 → 2325; fannkuch 2898 → 2086; nqueen 3319 → 2935. One got slower: mandelbrot 1691 → 1745, though its inner loop compiles to the same instructions, because the check on `inside++` had kept clang from setting each pixel's bit without a branch, and with a branch it was faster. matmul's 1173 → 1253 and base64's 300 → 325 are noise: timed side by side, the binaries before and after the change take the same time (1165 and 1163 ms, 301 and 301) |

The gaps that remain, largest first:

| benchmark | Zinkite / Goose | cause |
|---|---:|---|
| levenshtein | 1.9× | every index is checked against its view's length, and nothing proves them in range, where Goose's compiler does (`checkfree`): about 870 ms, measured with the checks taken out; `-fno-strict-aliasing` costs about 270 ms more |
| interp | 1.4× | not yet explained |
| records | 1.4× | no variants with payloads, so every event carries every variant's fields |
| base64 | 1.2× | not yet explained; 1.1× in the side-by-side timing above |
| words | 1.2× | not yet explained for the open-addressed table; the built-in hashmap's row is 2.0× Goose's std dictionary, from its layout: a lookup reads its index, then its entry, then the key, three cache misses where Goose's table makes two, a trade DESIGN §4.14 made knowingly |
| scene | 1.15× | its children are 8-byte references in an array, since links are only fields |
| fannkuch | 1.1× | checked indexing |
| matmul | 1.1× | Goose's `simd fn` runs AVX2 or AVX-512 copies of the kernel; the others are built for baseline x86-64 |

Most of what's left would come from proving index checks away, as Goose does, and from
variants with payloads (DESIGN §17).

Compiler bugs the first ports worked around, all fixed, with the ports changed to use what
they had worked around before the third run:
- `pushAll` on arrays passed the checker but wasn't in the code generator.
- A for-each over an array of inline strings (`u8[.*][..>]`) by reference generated C
  that didn't compile, and by `string` gave the element the wrong C type.
- A function returning an array of variable-size records generated C that didn't compile.
- Copying into a writable view (`a[:] = b`) generated C that didn't compile.
- A function that stored a reference parameter into a static array got a false
  lifetime error, reported against `zink/io/File.zn`.
- `long` as a name said "`i64` is a type, not a value" before it said `long` is reserved.
- A constructor that failed didn't reset the fields it set before failing (DESIGN §6.2).
- `i32[.16] p = [];` gave a fixed array of length 0, instead of 16 zeros.

## Files

| | |
|---|---|
| `run.py` | the harness |
| `benchmarks.py` | the manifest: each benchmark's sizes and implementations |
| `launch.c` | runs one program and reports its time and peak memory |
| `zinkite/`, `goose/`, `cpp/`, `rust/` | the implementations |
| `results.md`, `results.json` | the last run's results |
| `gen/` | build products (not committed) |
