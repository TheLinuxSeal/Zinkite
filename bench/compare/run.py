#!/usr/bin/env python3
"""Zinkite, Goose, C++ and Rust on the same benchmarks.

For every benchmark in the manifest, at every requested size, this builds each
implementation, runs it, and records the best wall time and the peak resident
memory. Every implementation of one benchmark must print the same lines; a
mismatch is reported and its time is marked, because a faster program that
computes something else isn't a result.

Timing is the whole process, including generating the input, building the
structures and tearing them down, as in Goose's own harness, whose manifest,
sizes and C++/Rust/Goose sources this suite reuses (see README.md).

  python3 bench/compare/run.py                         # everything
  python3 bench/compare/run.py --only sum,tree         # a subset; results.md keeps the rest
  python3 bench/compare/run.py --sizes small --reps 1  # a quick pass
  python3 bench/compare/run.py --report-only           # rewrite results.md from results.json

Needs clang and clang++ (or --cc/--cxx), rustc, a built zink (compiler/target/release
or debug), and a goose compiler (--goose, $GOOSE, or `goose` on PATH). A language
whose compiler is missing is skipped and says so.
"""

import argparse
import json
import math
import os
import platform
import re
import shutil
import subprocess
import sys
from datetime import datetime
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
GEN = HERE / "gen"
RESULTS_JSON = HERE / "results.json"
RESULTS_MD = HERE / "results.md"

SIZE_NAMES = ["small", "medium", "large"]
LANGS = ["zinkite", "goose", "cpp", "rust"]

# Where the size goes: the number on the one line marked `// BENCH_N`. C++ takes it
# as -DBENCH_N instead.
SIZE_LINE = {
    "zinkite": r"^(\s*i64\s+\w+\s*=\s*)(\d+)(\s*;.*//\s*BENCH_N.*)$",
    "goose": r"^(\s*let\s+\w+\s*=\s*)(\d+)(\s*;.*//\s*BENCH_N.*)$",
    "rust": r"^(\s*const\s+\w+\s*:\s*\w+\s*=\s*)(\d+)(\s*;.*//\s*BENCH_N.*)$",
}

# Address space each Goose data stack reserves: the 2 GB that Goose's harness uses.
GOOSE_STACK = "-DGS_STACK_RESERVE=2147483648ull"

from benchmarks import BENCHMARKS  # noqa: E402  (the manifest, beside this file)


# --- building ------------------------------------------------------------------

def sized_copy(src: Path, dst: Path, n: int, lang: str) -> None:
    """`src` with the number on its `// BENCH_N` line replaced by `n`."""
    pat = re.compile(SIZE_LINE[lang])
    out, hit = [], False
    for line in src.read_text().split("\n"):
        m = pat.match(line) if not hit else None
        if m:
            hit = True
            line = f"{m.group(1)}{n}{m.group(3)}"
        out.append(line)
    if not hit:
        raise SystemExit(f"no `// BENCH_N` line in {src}")
    dst.write_text("\n".join(out))


def run_quiet(cmd, log: Path, env=None) -> bool:
    r = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env)
    log.write_bytes(r.stdout)
    return r.returncode == 0


class Tools:
    def __init__(self, args):
        self.cc = args.cc
        self.cxx = args.cxx
        self.rustc = shutil.which(args.rustc) or shutil.which(
            str(Path.home() / ".cargo/bin/rustc"))
        # The newer of the release and debug builds, so a stale one isn't measured.
        built = [p for p in (REPO / "compiler/target/release/zink",
                             REPO / "compiler/target/debug/zink") if p.exists()]
        self.zink = args.zink or (str(max(built, key=lambda p: p.stat().st_mtime))
                                  if built else None)
        self.goose = args.goose or os.environ.get("GOOSE") or shutil.which("goose")
        self.missing = {}
        if not self.zink:
            self.missing["zinkite"] = "no zink: run `cargo build --release` in compiler/"
        if not self.goose:
            self.missing["goose"] = "no goose compiler: pass --goose or set $GOOSE"
        if not self.rustc:
            self.missing["rust"] = "no rustc"
        if not shutil.which(self.cxx):
            self.missing["cpp"] = f"no {self.cxx}"

    def version(self, cmd):
        try:
            r = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
            return r.stdout.strip().split("\n")[0]
        except OSError:
            return None

    def build(self, lang, impl, n, tag):
        """The executable for one implementation at size `n`, or (None, why)."""
        exe = GEN / tag
        log = GEN / f"{tag}.log"
        if lang == "zinkite":
            src = GEN / f"{tag}.zn"
            sized_copy(HERE / "zinkite" / impl["file"], src, n, lang)
            env = dict(os.environ, CC=self.cc)
            ok = run_quiet([self.zink, "build", str(src), "-o", str(exe)], log, env)
        elif lang == "goose":
            # Goose finds `import rng;` beside the file it compiles.
            d = GEN / f"{tag}.goose.d"
            d.mkdir(exist_ok=True)
            shutil.copy(HERE / "goose" / "rng.goose", d / "rng.goose")
            src = d / impl["file"]
            sized_copy(HERE / "goose" / impl["file"], src, n, lang)
            c = GEN / f"{tag}.c"
            ok = run_quiet([self.goose, "-O2", "--standalone", "-o", str(c), str(src)], log)
            ok = ok and run_quiet([self.cc, "-O2", GOOSE_STACK, str(c), "-o", str(exe), "-lm"],
                                  GEN / f"{tag}.cc.log")
        elif lang == "cpp":
            ok = run_quiet([self.cxx, "-O2", "-std=c++20", f"-DBENCH_N={n}",
                            f"-DVARIANT={impl.get('variant', 0)}", "-I", str(HERE / "cpp"),
                            str(HERE / "cpp" / impl["file"]), "-o", str(exe)], log)
        else:
            # rustc -O with one codegen unit: one whole translation unit, as the C++
            # rows are; everything else at the defaults (bounds checks on).
            d = GEN / f"{tag}.rs.d"
            d.mkdir(exist_ok=True)
            shutil.copy(HERE / "rust" / "bench.rs", d / "bench.rs")
            src = d / impl["file"]
            sized_copy(HERE / "rust" / impl["file"], src, n, lang)
            ok = run_quiet([self.rustc, "-O", "-C", "codegen-units=1", "--edition", "2021",
                            "-o", str(exe), str(src)], log)
        return (exe, None) if ok else (None, f"build failed, see {log.relative_to(HERE)}")


# --- measuring -----------------------------------------------------------------

LAUNCH = GEN / "launch"


def build_launcher(cc):
    """launch.c, which runs a program and reports its time and peak memory."""
    src = HERE / "launch.c"
    if not LAUNCH.exists() or LAUNCH.stat().st_mtime < src.stat().st_mtime:
        subprocess.run([cc, "-O2", str(src), "-o", str(LAUNCH)], check=True)


def run_once(exe):
    """Wall time in ms, peak RSS in bytes, stdout, and exit status of one run."""
    r = subprocess.run([str(LAUNCH), str(exe)], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    ms, kb, code = r.stderr.decode().split()[-3:]
    return float(ms), int(kb) * 1024, r.stdout.decode(errors="replace"), int(code)


def measure(exe, reps, warmup):
    for _ in range(warmup):
        run_once(exe)
    best, peak, out = None, 0, None
    for _ in range(reps):
        ms, rss, o, code = run_once(exe)
        if code != 0:
            return {"ok": False, "why": f"exit status {code}"}
        best = ms if best is None else min(best, ms)
        peak = max(peak, rss)
        out = o
    return {"ok": True, "ms": best, "peak": peak, "out": out}


# --- the report ----------------------------------------------------------------

def fmt_ms(m):
    if not m or not m.get("ok"):
        return "--"
    s = f"{m['ms']:.0f}" if round(m["ms"], 1) >= 100 else f"{m['ms']:.1f}"
    return s + (" ✗" if m.get("mismatch") else "")


def fmt_mb(m):
    return f"{m['peak'] / (1 << 20):.1f}" if m and m.get("ok") else "--"


def geomean(xs):
    xs = [x for x in xs if x and x > 0]
    return math.exp(sum(math.log(x) for x in xs) / len(xs)) if xs else None


def impls(b):
    for lang in LANGS:
        for impl in b.get(lang, []):
            yield lang, impl


def best_of(results, b, lang, size, tier=None):
    """The fastest passing run of one language at one size, or None."""
    best = None
    for l, impl in impls(b):
        if l != lang or (tier and impl.get("tier") != tier):
            continue
        m = results.get(b["name"], {}).get(impl["label"], {}).get(size)
        if m and m.get("ok") and not m.get("mismatch"):
            if best is None or m["ms"] < best["ms"]:
                best = m
    return best


def write_report(results, env):
    lines = []
    W = lines.append
    W("# Zinkite, Goose, C++ and Rust")
    W("")
    W(f"Generated by `bench/compare/run.py` on {env['date']}. Don't edit by hand.")
    W("")
    W("Each time is the best of several whole-process runs, after a discarded warm-up: it")
    W("includes generating the input, building the structures and tearing them down. Memory")
    W("is the peak resident set at the largest size. Every implementation of a benchmark")
    W("prints the same checksum lines; a time marked ✗ printed something else and is left out")
    W("of the comparisons.")
    W("")
    W("## Environment")
    W("")
    W("| | |")
    W("|---|---|")
    for k in ["cpu", "os", "cc", "cxx", "rustc", "zink", "goose"]:
        if env.get(k):
            W(f"| {k} | {env[k]} |")
    W("")
    W("C++ and the C that Zinkite and Goose generate are built by the same clang at -O2.")
    W("Rust is `rustc -O -C codegen-units=1`.")
    W("")

    # The summary: each language's best row at the largest size measured.
    W("## Across all benchmarks")
    W("")
    W("Each language's fastest implementation at the largest size measured, in ms, and how")
    W("many times longer it took than Zinkite's (above 1.00 means Zinkite is faster). *C++")
    W("idiomatic* is what a C++ programmer writes first, *C++ expert* the hand-tuned version.")
    W("")
    W("| benchmark | size | Zinkite | Goose | C++ idiomatic | C++ expert | Rust | Goose / Z | C++ id. / Z | C++ exp. / Z | Rust / Z |")
    W("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
    ratios = {"goose": [], "idiomatic": [], "expert": [], "rust": []}
    for b in BENCHMARKS:
        r = results.get(b["name"])
        if not r:
            continue
        size = next((s for s in reversed(SIZE_NAMES)
                     if any(s in v for v in r.values())), None)
        if not size:
            continue
        z = best_of(results, b, "zinkite", size)
        cols = {
            "goose": best_of(results, b, "goose", size),
            "idiomatic": best_of(results, b, "cpp", size, "idiomatic"),
            "expert": best_of(results, b, "cpp", size, "expert"),
            "rust": best_of(results, b, "rust", size),
        }
        row = f"| {b['name']} | {size} | {fmt_ms(z)} |"
        for k in ["goose", "idiomatic", "expert", "rust"]:
            row += f" {fmt_ms(cols[k])} |"
        for k in ["goose", "idiomatic", "expert", "rust"]:
            if z and cols[k]:
                x = cols[k]["ms"] / z["ms"]
                ratios[k].append(x)
                row += f" {x:.2f}x |"
            else:
                row += " -- |"
        W(row)
    g = {k: geomean(v) for k, v in ratios.items()}
    W("| **geometric mean** | | | | | | | "
      + " | ".join(f"**{g[k]:.2f}x**" if g[k] else "--"
                   for k in ["goose", "idiomatic", "expert", "rust"]) + " |")
    W("")

    W("## Each benchmark")
    W("")
    for b in BENCHMARKS:
        r = results.get(b["name"])
        if not r:
            continue
        W(f"### {b['name']}")
        W("")
        W(b["what"])
        W("")
        p = b.get("param", "N")
        head = " | ".join(f"{p}={n}" for n in b["sizes"])
        W(f"| implementation | {head} | MB at {p}={b['sizes'][-1]} |")
        W("|---|" + "---:|" * (len(b["sizes"]) + 1))
        for lang, impl in impls(b):
            m = r.get(impl["label"], {})
            if not m:
                continue
            cells = " | ".join(fmt_ms(m.get(s)) for s in SIZE_NAMES[:len(b["sizes"])])
            W(f"| {impl['label']} | {cells} | {fmt_mb(m.get('large'))} |")
        notes = [f"{impl['label']}: {m[s]['why']}" for lang, impl in impls(b)
                 for m in [r.get(impl["label"], {})] for s in SIZE_NAMES
                 if s in m and not m[s].get("ok")]
        if notes:
            W("")
            W("Failed: " + "; ".join(sorted(set(notes))) + ".")
        if b.get("note"):
            W("")
            W(b["note"])
        W("")
    RESULTS_MD.write_text("\n".join(lines) + "\n")
    write_full_table(results, env)


def size_text(n):
    """A size as written in the table: 16000000 is 16M, 1600000 is 1.6M, 2048 stays 2048."""
    if n >= 1_000_000 and n % 100_000 == 0:
        return f"{n / 1_000_000:g}M"
    if n >= 10_000 and n % 1_000 == 0:
        return f"{n // 1_000}k"
    return str(n)


def write_full_table(results, env):
    """BENCHMARKS.md at the top of the repo: every implementation's time and peak memory
    at every size, a column per language."""
    names = {"zinkite": "Zinkite", "goose": "Goose", "cpp": "C++", "rust": "Rust"}

    def label(lang, impl):
        # The language is the column, so the label loses its first word; `<` is escaped
        # so `vector<string>` isn't read as HTML.
        text = impl["label"]
        if text.lower().startswith(lang + " "):
            text = text[len(lang) + 1:]
        return text.replace("<", "\\<")

    def table(cell):
        W("| benchmark | sizes | " + " | ".join(names[l] for l in LANGS) + " |")
        W("|---|---|" + "---|" * len(LANGS))
        for b in BENCHMARKS:
            r = results.get(b["name"])
            if not r:
                continue
            p = b.get("param", "N")
            sizes = f"{p} = " + ", ".join(size_text(n) for n in b["sizes"])
            rows = max(len(b.get(l, [])) for l in LANGS)
            for k in range(rows):
                cells = []
                for lang in LANGS:
                    impls_l = b.get(lang, [])
                    if k >= len(impls_l):
                        cells.append("")
                        continue
                    m = r.get(impls_l[k]["label"], {})
                    vals = ", ".join(cell(m.get(s)) for s in SIZE_NAMES)
                    cells.append(f"{label(lang, impls_l[k])}: {vals}")
                name, sz = (f"**{b['name']}**", sizes) if k == 0 else ("", "")
                W(f"| {name} | {sz} | " + " | ".join(cells) + " |")
        W("")

    lines = []
    W = lines.append
    W("# Benchmarks")
    W("")
    W("Every implementation of the 26 benchmarks in [`bench/compare`](bench/compare/README.md),")
    W("a column per language. Each cell is one implementation: its name, then its numbers at")
    W("the small, medium and large sizes, which are in the sizes column. Generated by")
    W(f"`bench/compare/run.py` on {env['date']}; the comparisons and notes are in")
    W("[`bench/compare/results.md`](bench/compare/results.md).")
    W("")
    W(f"Machine: {env.get('cpu')}, {env.get('os')}. C++ and the C that Zinkite and Goose")
    W("generate are built by clang at -O2; Rust by `rustc -O -C codegen-units=1`. Each time is")
    W("the best of three whole-process runs after a warm-up. A time marked ✗ printed")
    W("something different from the other implementations.")
    W("")
    W("## Time (ms)")
    W("")
    table(fmt_ms)
    W("## Peak memory (MB)")
    W("")
    table(fmt_mb)
    (REPO / "BENCHMARKS.md").write_text("\n".join(lines))


def environment(tools):
    cpu = None
    try:
        for line in Path("/proc/cpuinfo").read_text().split("\n"):
            if line.startswith("model name"):
                cpu = line.split(":", 1)[1].strip()
                break
    except OSError:
        pass
    return {
        "date": datetime.now().strftime("%Y-%m-%d %H:%M"),
        "cpu": cpu,
        "os": f"{platform.system()} {platform.release()}",
        "cc": tools.version([tools.cc, "--version"]),
        "cxx": tools.version([tools.cxx, "--version"]),
        "rustc": tools.version([tools.rustc, "--version"]) if tools.rustc else None,
        "zink": tools.zink and f"zink at {subprocess.run(['git', '-C', str(REPO), 'rev-parse', '--short', 'HEAD'], stdout=subprocess.PIPE, text=True).stdout.strip()}",
        "goose": tools.version([tools.goose, "version"]) if tools.goose else None,
    }


# --- main ----------------------------------------------------------------------

def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--only", help="comma-separated benchmark names")
    ap.add_argument("--langs", default=",".join(LANGS), help="comma-separated languages")
    ap.add_argument("--sizes", default=",".join(SIZE_NAMES), help="small,medium,large")
    ap.add_argument("--reps", type=int, default=3, help="timed runs per cell (best is kept)")
    ap.add_argument("--warmup", type=int, default=1, help="discarded runs before timing")
    ap.add_argument("--cc", default="clang")
    ap.add_argument("--cxx", default="clang++")
    ap.add_argument("--rustc", default="rustc")
    ap.add_argument("--zink")
    ap.add_argument("--goose")
    ap.add_argument("--report-only", action="store_true")
    args = ap.parse_args()

    results = json.loads(RESULTS_JSON.read_text()) if RESULTS_JSON.exists() else {}
    tools = Tools(args)
    saved_env = results.pop("_env", None)
    if args.report_only:
        write_report(results, saved_env or environment(tools))
        print(f"wrote {RESULTS_MD.relative_to(REPO)}")
        return

    GEN.mkdir(exist_ok=True)
    build_launcher(args.cc)
    only = set(args.only.split(",")) if args.only else None
    langs = [l for l in args.langs.split(",") if l]
    sizes = [s for s in args.sizes.split(",") if s]
    for l, why in tools.missing.items():
        if l in langs:
            print(f"skipping {l}: {why}")
    names = {b["name"] for b in BENCHMARKS}
    for n in only or ():
        if n not in names:
            raise SystemExit(f"no benchmark {n!r}; there are: {', '.join(sorted(names))}")

    for b in BENCHMARKS:
        if only and b["name"] not in only:
            continue
        r = results.setdefault(b["name"], {})
        for si, size in enumerate(SIZE_NAMES):
            if size not in sizes or si >= len(b["sizes"]):
                continue
            n = b["sizes"][si]
            print(f"{b['name']} {b.get('param', 'N')}={n}")
            outs = {}
            for lang, impl in impls(b):
                if lang not in langs or lang in tools.missing:
                    continue
                tag = f"{b['name']}_{size}_{lang}_{Path(impl['file']).stem}_v{impl.get('variant', 0)}"
                exe, why = tools.build(lang, impl, n, tag)
                m = measure(exe, args.reps, args.warmup) if exe else {"ok": False, "why": why}
                r.setdefault(impl["label"], {})[size] = m
                if m["ok"]:
                    outs[impl["label"]] = m["out"]
                    print(f"  {impl['label']:<44} {m['ms']:>9.1f} ms {m['peak'] / (1 << 20):>8.1f} MB")
                else:
                    print(f"  {impl['label']:<44} {m['why']}")
            # Every implementation must print the same thing: the most common output
            # is taken as right, and any other is marked.
            if outs:
                counts = {}
                for o in outs.values():
                    counts[o] = counts.get(o, 0) + 1
                right = max(counts, key=counts.get)
                for label, o in outs.items():
                    m = r[label][size]
                    m["mismatch"] = o != right
                    if m["mismatch"]:
                        print(f"  MISMATCH {label}: {o.strip()!r} against {right.strip()!r}")
            results["_env"] = environment(tools)
            RESULTS_JSON.write_text(json.dumps(results, indent=1))
            results.pop("_env")
    env = environment(tools)
    results["_env"] = env
    RESULTS_JSON.write_text(json.dumps(results, indent=1))
    results.pop("_env")
    write_report(results, env)
    print(f"wrote {RESULTS_MD.relative_to(REPO)}")


if __name__ == "__main__":
    main()
